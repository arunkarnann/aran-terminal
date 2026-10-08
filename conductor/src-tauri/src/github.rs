//! GitHub issues sidebar: account discovery, token lookup, REST/GraphQL fetches and
//! the pure parsers behind them.
//!
//! Tokens are never persisted by us and never put on a command line: they come from
//! `gh auth token` (stdout) or the macOS Keychain (pasted PATs), live in memory for one
//! request batch, and travel only as an HTTP header via the in-process `ureq` client.
//! Error strings never include headers or tokens.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};

use crate::ipc::{GhField, GhItem, GhLabel, GhProjectInfo, GhProjectList, GhRepoInfo};

const API: &str = "https://api.github.com";
const KEYCHAIN_SERVICE: &str = "aran-terminal-github";
const USER_AGENT: &str = "aran-terminal";
/// Pagination caps (100 per page) so a huge org can't stall a sync.
const MAX_REPO_PAGES: usize = 5;
const MAX_ISSUE_PAGES: usize = 3;
const MAX_PROJECT_ITEM_PAGES: usize = 5;

// ---- gh CLI discovery -------------------------------------------------------

/// Locate `gh`. Apps launched from Finder get a minimal PATH without Homebrew, so the
/// usual install locations are probed before falling back to PATH.
pub fn gh_path() -> Option<PathBuf> {
    resolve_gh(
        &["/opt/homebrew/bin/gh", "/usr/local/bin/gh"],
        std::env::var_os("PATH").as_deref(),
    )
}

fn resolve_gh(candidates: &[&str], path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    for c in candidates {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Some(p);
        }
    }
    let path_var = path_var?;
    std::env::split_paths(path_var)
        .map(|d| d.join("gh"))
        .find(|p| p.is_file())
}

/// One account parsed from `gh auth status`.
#[derive(Debug, Clone, PartialEq)]
pub struct GhCliAccount {
    pub login: String,
    pub active: bool,
    /// None when gh doesn't report scopes (e.g. fine-grained tokens).
    pub scopes: Option<Vec<String>>,
}

/// Parse the human output of `gh auth status` (github.com host only). Token lines are
/// masked by gh and ignored here.
pub fn parse_auth_status(text: &str) -> Vec<GhCliAccount> {
    let mut out: Vec<GhCliAccount> = Vec::new();
    let mut host_ok = true;
    for raw in text.lines() {
        let line = raw.trim();
        if !raw.starts_with(' ') && !line.is_empty() {
            // Host header line, e.g. "github.com".
            host_ok = line == "github.com";
            continue;
        }
        if !host_ok {
            continue;
        }
        if let Some(i) = line.find("account ") {
            if line.contains("Logged in to") {
                let rest = &line[i + "account ".len()..];
                let login = rest.split_whitespace().next().unwrap_or("").to_string();
                if !login.is_empty() {
                    out.push(GhCliAccount { login, active: false, scopes: None });
                }
                continue;
            }
        }
        let Some(cur) = out.last_mut() else { continue };
        if let Some(v) = line.strip_prefix("- Active account:") {
            cur.active = v.trim() == "true";
        } else if let Some(v) = line.strip_prefix("- Token scopes:") {
            let scopes: Vec<String> = v
                .split(',')
                .map(|s| s.trim().trim_matches('\'').to_string())
                .filter(|s| !s.is_empty() && s != "none")
                .collect();
            cur.scopes = Some(scopes);
        }
    }
    out
}

/// Accounts known to the gh CLI. Empty if gh isn't installed. gh exits non-zero when any
/// account has a problem, so the exit code is ignored and both streams are parsed.
pub fn gh_cli_accounts() -> Vec<GhCliAccount> {
    let Some(gh) = gh_path() else { return Vec::new() };
    let Ok(out) = Command::new(gh).args(["auth", "status", "--hostname", "github.com"]).output() else {
        return Vec::new();
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    parse_auth_status(&text)
}

/// True if this scope list allows reading Projects v2.
pub fn scopes_allow_projects(scopes: &[String]) -> bool {
    scopes.iter().any(|s| s == "read:project" || s == "project")
}

/// Token for a gh-managed account, read from gh's stdout (never logged).
pub fn gh_token(login: &str) -> Result<String, String> {
    let gh = gh_path().ok_or("GitHub CLI (gh) not found")?;
    let out = Command::new(gh)
        .args(["auth", "token", "--hostname", "github.com", "--user", login])
        .output()
        .map_err(|e| format!("could not run gh: {e}"))?;
    let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || tok.is_empty() {
        return Err(format!("gh has no token for {login}; run `gh auth login`"));
    }
    Ok(tok)
}

// ---- Keychain (pasted personal access tokens) --------------------------------

fn keychain_entry(login: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, login).map_err(|e| format!("keychain: {e}"))
}

pub fn keychain_set(login: &str, token: &str) -> Result<(), String> {
    keychain_entry(login)?
        .set_password(token)
        .map_err(|e| format!("keychain: {e}"))
}

pub fn keychain_get(login: &str) -> Result<String, String> {
    keychain_entry(login)?
        .get_password()
        .map_err(|_| format!("no saved token for {login}; add it again"))
}

pub fn keychain_delete(login: &str) {
    if let Ok(e) = keychain_entry(login) {
        let _ = e.delete_credential();
    }
}

// ---- HTTP --------------------------------------------------------------------

pub struct Client {
    agent: ureq::Agent,
    token: String,
}

/// Turn a ureq error into a short, token-free message.
fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let msg = resp
                .into_json::<Value>()
                .ok()
                .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            match code {
                401 => "GitHub rejected the token (401). Re-authenticate this account.".into(),
                403 if msg.to_lowercase().contains("rate limit") => {
                    "GitHub rate limit reached; try again later.".into()
                }
                404 => "Not found (404) — no access, or it was removed.".into(),
                _ => format!("GitHub error {code}: {msg}"),
            }
        }
        ureq::Error::Transport(t) => format!("network error: {}", t.kind()),
    }
}

/// `rel="next"` target from a Link header.
fn next_link(link: Option<&str>) -> Option<String> {
    link?.split(',').find_map(|part| {
        let (url, rel) = part.split_once(';')?;
        rel.contains("rel=\"next\"")
            .then(|| url.trim().trim_start_matches('<').trim_end_matches('>').to_string())
    })
}

impl Client {
    pub fn new(token: String) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .user_agent(USER_AGENT)
            .build();
        Client { agent, token }
    }

    fn auth(&self, req: ureq::Request) -> ureq::Request {
        req.set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
    }

    /// GET returning (json, next page url, X-OAuth-Scopes).
    fn get(&self, url: &str) -> Result<(Value, Option<String>, Option<String>), String> {
        let resp = self.auth(self.agent.get(url)).call().map_err(http_err)?;
        let next = next_link(resp.header("link"));
        let scopes = resp.header("x-oauth-scopes").map(str::to_string);
        let v = resp.into_json::<Value>().map_err(|e| format!("bad response: {e}"))?;
        Ok((v, next, scopes))
    }

    /// GET following Link pagination up to `max_pages`, concatenating arrays.
    fn get_paged(&self, first: &str, max_pages: usize) -> Result<Vec<Value>, String> {
        let mut out = Vec::new();
        let mut url = Some(first.to_string());
        for _ in 0..max_pages {
            let Some(u) = url.take() else { break };
            let (v, next, _) = self.get(&u)?;
            if let Value::Array(items) = v {
                out.extend(items);
            }
            url = next;
        }
        Ok(out)
    }

    /// GraphQL POST. Returns (data, error messages) — GitHub may return partial data
    /// alongside errors (e.g. an org behind SAML SSO), so both are kept.
    fn graphql(&self, query: &str, variables: Value) -> Result<(Value, Vec<GqlError>), String> {
        let resp = self
            .auth(self.agent.post(&format!("{API}/graphql")))
            .send_json(json!({ "query": query, "variables": variables }))
            .map_err(http_err)?;
        let v = resp.into_json::<Value>().map_err(|e| format!("bad response: {e}"))?;
        Ok(split_graphql(v))
    }

    /// Login + scopes for the token (scopes None for fine-grained tokens).
    pub fn whoami(&self) -> Result<(String, Option<Vec<String>>), String> {
        let (v, _, scopes) = self.get(&format!("{API}/user"))?;
        let login = v
            .get("login")
            .and_then(Value::as_str)
            .ok_or("unexpected /user response")?
            .to_string();
        let scopes = scopes.map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        });
        Ok((login, scopes))
    }

    pub fn list_repos(&self) -> Result<Vec<GhRepoInfo>, String> {
        let items = self.get_paged(
            &format!(
                "{API}/user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member"
            ),
            MAX_REPO_PAGES,
        )?;
        Ok(items.iter().filter_map(parse_repo).collect())
    }

    pub fn list_projects(&self) -> Result<GhProjectList, String> {
        let (data, errors) = self.graphql(&PROJECTS_QUERY, json!({}))?;
        if errors.iter().any(|e| e.kind == "INSUFFICIENT_SCOPES") {
            return Err(MISSING_PROJECT_SCOPE.into());
        }
        if data.is_null() {
            return Err(errors
                .first()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "GitHub returned no data".into()));
        }
        let mut list = parse_projects(&data);
        list.notices.extend(errors.into_iter().map(|e| e.message));

        // `viewer.organizations` only lists orgs whose membership GitHub exposes to this
        // token (private membership / OAuth app restrictions can hide them all), so also
        // ask every org that owns a repo we can see, plus /user/orgs, directly.
        let mut seen: Vec<String> = data
            .pointer("/viewer/organizations/nodes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|o| s(o, "/login")).collect())
            .unwrap_or_default();
        let mut extra: Vec<String> = Vec::new();
        let repo_owners = self
            .get_paged(
                &format!("{API}/user/repos?per_page=100&affiliation=collaborator,organization_member"),
                MAX_REPO_PAGES,
            )
            .unwrap_or_default();
        let user_orgs = self.get_paged(&format!("{API}/user/orgs?per_page=100"), 1).unwrap_or_default();
        for login in org_logins(&repo_owners, &user_orgs) {
            if !seen.iter().any(|x| x.eq_ignore_ascii_case(&login)) {
                seen.push(login.clone());
                extra.push(login);
            }
        }
        for chunk in extra.chunks(10) {
            let (query, vars) = org_projects_query(chunk);
            let (data, errors) = self.graphql(&query, vars)?;
            let found = parse_org_projects(&data);
            for p in found {
                if !list.projects.iter().any(|x| x.id == p.id) {
                    list.projects.push(p);
                }
            }
            list.notices.extend(errors.into_iter().map(|e| e.message));
        }
        Ok(list)
    }

    /// Open issues + PRs of one repo, most recently updated first.
    /// Open issues + PRs of one repo via GraphQL (REST has no last-comment/edit times).
    pub fn repo_items(&self, source_id: &str, full_name: &str) -> Result<Vec<GhItem>, String> {
        let (owner, name) = full_name
            .split_once('/')
            .ok_or_else(|| format!("bad repo name: {full_name}"))?;
        let mut out = Vec::new();
        for (query, conn, kind) in [
            (&*REPO_ISSUES_QUERY, "issues", "issue"),
            (&*REPO_PRS_QUERY, "pullRequests", "pr"),
        ] {
            let mut cursor: Option<String> = None;
            for _ in 0..MAX_ISSUE_PAGES {
                let (data, errors) =
                    self.graphql(query, json!({ "owner": owner, "name": name, "after": cursor }))?;
                let Some(page) = data.pointer(&format!("/repository/{conn}")).filter(|p| !p.is_null()) else {
                    return Err(errors
                        .first()
                        .map(|e| e.message.clone())
                        .unwrap_or_else(|| "repository not found".into()));
                };
                if let Some(nodes) = page.get("nodes").and_then(Value::as_array) {
                    out.extend(nodes.iter().filter_map(|c| {
                        let num = c.get("number")?.as_i64()?;
                        Some(content_to_item(
                            c,
                            source_id,
                            kind,
                            format!("{full_name}#{num}"),
                            None,
                            Vec::new(),
                        ))
                    }));
                }
                let more = page.pointer("/pageInfo/hasNextPage").and_then(Value::as_bool).unwrap_or(false);
                cursor = s(page, "/pageInfo/endCursor");
                if !more || cursor.is_none() {
                    break;
                }
            }
        }
        out.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
        Ok(out)
    }

    /// Items of a Projects v2 board plus its status column order.
    pub fn project_items(
        &self,
        source_id: &str,
        project_id: &str,
    ) -> Result<(Vec<GhItem>, Vec<String>), String> {
        let mut items = Vec::new();
        let mut status_field: Option<(String, Vec<String>)> = None;
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PROJECT_ITEM_PAGES {
            let (data, errors) = self.graphql(
                &PROJECT_ITEMS_QUERY,
                json!({ "id": project_id, "after": cursor }),
            )?;
            if errors.iter().any(|e| e.kind == "INSUFFICIENT_SCOPES") {
                return Err(MISSING_PROJECT_SCOPE.into());
            }
            let project = data.pointer("/node");
            let Some(project) = project.filter(|p| !p.is_null()) else {
                return Err(errors
                    .first()
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "project not found".into()));
            };
            if status_field.is_none() {
                status_field = Some(pick_status_field(project));
            }
            let status_name = status_field.as_ref().map(|(n, _)| n.as_str()).unwrap_or("");
            if let Some(nodes) = project.pointer("/items/nodes").and_then(Value::as_array) {
                items.extend(nodes.iter().filter_map(|n| parse_project_item(n, source_id, status_name)));
            }
            let has_next = project
                .pointer("/items/pageInfo/hasNextPage")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            cursor = project
                .pointer("/items/pageInfo/endCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if !has_next || cursor.is_none() {
                break;
            }
        }
        Ok((items, status_field.map(|(_, o)| o).unwrap_or_default()))
    }
}

pub const MISSING_PROJECT_SCOPE: &str = "This token can't read GitHub Projects. Run in a terminal: \
     gh auth refresh -s read:project --user <account>";

// ---- GraphQL ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct GqlError {
    pub kind: String,
    pub message: String,
}

fn split_graphql(v: Value) -> (Value, Vec<GqlError>) {
    let errors = v
        .get("errors")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|e| GqlError {
                    kind: e.get("type").and_then(Value::as_str).unwrap_or("").to_string(),
                    message: e.get("message").and_then(Value::as_str).unwrap_or("error").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    (v.get("data").cloned().unwrap_or(Value::Null), errors)
}

const PROJECT_FIELDS: &str = "id title number url closed owner { ... on User { login } ... on Organization { login } }";

static PROJECTS_QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "query {{ viewer {{ projectsV2(first: 50) {{ nodes {{ {PROJECT_FIELDS} }} }} \
         organizations(first: 50) {{ nodes {{ login projectsV2(first: 50) {{ nodes {{ {PROJECT_FIELDS} }} }} }} }} }} }}"
    )
});

/// Issue/PR fields shared by the repo and project queries.
const CONTENT_FIELDS: &str = "number title url state body createdAt updatedAt lastEditedAt \
     author { login } assignees(first: 5) { nodes { login } } \
     labels(first: 10) { nodes { name color } } \
     comments(last: 1) { totalCount nodes { createdAt author { login } } } \
     repository { nameWithOwner }";

static REPO_ISSUES_QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "query($owner: String!, $name: String!, $after: String) {{ repository(owner: $owner, name: $name) {{ \
         issues(states: OPEN, first: 100, after: $after, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ \
         pageInfo {{ hasNextPage endCursor }} nodes {{ {CONTENT_FIELDS} }} }} }} }}"
    )
});

static REPO_PRS_QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "query($owner: String!, $name: String!, $after: String) {{ repository(owner: $owner, name: $name) {{ \
         pullRequests(states: OPEN, first: 100, after: $after, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ \
         pageInfo {{ hasNextPage endCursor }} nodes {{ {CONTENT_FIELDS} }} }} }} }}"
    )
});

static PROJECT_ITEMS_QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        r#"
query($id: ID!, $after: String) {{
  node(id: $id) {{
    ... on ProjectV2 {{
      fields(first: 30) {{
        nodes {{ ... on ProjectV2SingleSelectField {{ name options {{ name }} }} }}
      }}
      items(first: 100, after: $after) {{
        pageInfo {{ hasNextPage endCursor }}
        nodes {{
          id
          type
          fieldValues(first: 20) {{
            nodes {{
              ... on ProjectV2ItemFieldSingleSelectValue {{ name field {{ ... on ProjectV2FieldCommon {{ name }} }} }}
              ... on ProjectV2ItemFieldIterationValue {{ title field {{ ... on ProjectV2FieldCommon {{ name }} }} }}
              ... on ProjectV2ItemFieldTextValue {{ text field {{ ... on ProjectV2FieldCommon {{ name }} }} }}
              ... on ProjectV2ItemFieldNumberValue {{ number field {{ ... on ProjectV2FieldCommon {{ name }} }} }}
              ... on ProjectV2ItemFieldDateValue {{ date field {{ ... on ProjectV2FieldCommon {{ name }} }} }}
            }}
          }}
          content {{
            ... on Issue {{ {CONTENT_FIELDS} }}
            ... on PullRequest {{ {CONTENT_FIELDS} }}
            ... on DraftIssue {{ title body createdAt updatedAt }}
          }}
        }}
      }}
    }}
  }}
}}"#
    )
});

// ---- Parsers (pure; unit-tested) ---------------------------------------------

fn s(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(Value::as_str).map(str::to_string)
}

/// ISO-8601 UTC ("2026-10-08T05:12:34Z") → epoch ms. 0 if unparsable.
pub fn iso_to_ms(iso: &str) -> i64 {
    let b = iso.as_bytes();
    if b.len() < 19 {
        return 0;
    }
    let num = |a: usize, z: usize| iso.get(a..z).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) =
        (num(0, 4), num(5, 7), num(8, 10), num(11, 13), num(14, 16), num(17, 19))
    else {
        return 0;
    };
    // Days from civil (Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 86_400) + h * 3_600 + mi * 60 + se) * 1_000
}

/// Label colors arrive as 6-hex without '#'; anything else is dropped (never trusted
/// into a style attribute).
fn safe_color(c: Option<&str>) -> Option<String> {
    let c = c?.trim_start_matches('#');
    (c.len() == 6 && c.chars().all(|ch| ch.is_ascii_hexdigit())).then(|| format!("#{}", c.to_lowercase()))
}

fn labels_from(arr: Option<&Value>) -> Vec<GhLabel> {
    arr.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    Some(GhLabel {
                        name: l.get("name")?.as_str()?.to_string(),
                        color: safe_color(l.get("color").and_then(Value::as_str)),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn logins_from(arr: Option<&Value>) -> Vec<String> {
    arr.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("login").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_repo(v: &Value) -> Option<GhRepoInfo> {
    Some(GhRepoInfo {
        full_name: s(v, "/full_name")?,
        url: s(v, "/html_url").unwrap_or_default(),
        private: v.get("private").and_then(Value::as_bool).unwrap_or(false),
        description: s(v, "/description"),
        pushed_at: s(v, "/pushed_at").map(|p| iso_to_ms(&p)),
    })
}

/// GraphQL Issue / PullRequest / DraftIssue content → item.
pub fn content_to_item(
    c: &Value,
    source_id: &str,
    kind: &str,
    item_key: String,
    status: Option<String>,
    fields: Vec<GhField>,
) -> GhItem {
    let ts = |ptr: &str| s(c, ptr).map(|t| iso_to_ms(&t)).filter(|&ms| ms > 0);
    GhItem {
        source_id: source_id.to_string(),
        item_key,
        kind: kind.into(),
        repo: s(c, "/repository/nameWithOwner"),
        number: c.get("number").and_then(Value::as_i64),
        title: s(c, "/title").unwrap_or_default(),
        state: s(c, "/state").unwrap_or_else(|| "open".into()).to_lowercase(),
        url: s(c, "/url"),
        author: s(c, "/author/login"),
        assignees: logins_from(c.pointer("/assignees/nodes")),
        labels: labels_from(c.pointer("/labels/nodes")),
        comments: c.pointer("/comments/totalCount").and_then(Value::as_i64).unwrap_or(0),
        updated_at: ts("/updatedAt").unwrap_or(0),
        status,
        fields,
        body: s(c, "/body").unwrap_or_default(),
        created_at: ts("/createdAt").unwrap_or(0),
        edited_at: ts("/lastEditedAt"),
        last_comment_at: ts("/comments/nodes/0/createdAt"),
        last_comment_by: s(c, "/comments/nodes/0/author/login"),
    }
}

pub fn parse_projects(data: &Value) -> GhProjectList {
    let mut projects = Vec::new();
    let mut push = |n: &Value| {
        let (Some(id), Some(title)) = (s(n, "/id"), s(n, "/title")) else { return };
        if n.get("closed").and_then(Value::as_bool).unwrap_or(false) {
            return;
        }
        if projects.iter().any(|p: &GhProjectInfo| p.id == id) {
            return;
        }
        projects.push(GhProjectInfo {
            id,
            title,
            number: n.get("number").and_then(Value::as_i64).unwrap_or(0),
            owner: s(n, "/owner/login").unwrap_or_default(),
            url: s(n, "/url").unwrap_or_default(),
        });
    };
    if let Some(nodes) = data.pointer("/viewer/projectsV2/nodes").and_then(Value::as_array) {
        nodes.iter().filter(|n| !n.is_null()).for_each(&mut push);
    }
    if let Some(orgs) = data.pointer("/viewer/organizations/nodes").and_then(Value::as_array) {
        for org in orgs.iter().filter(|o| !o.is_null()) {
            if let Some(nodes) = org.pointer("/projectsV2/nodes").and_then(Value::as_array) {
                nodes.iter().filter(|n| !n.is_null()).for_each(&mut push);
            }
        }
    }
    GhProjectList { projects, notices: Vec::new() }
}

/// Org logins from REST repo objects (owner.type == "Organization") and /user/orgs,
/// de-duplicated case-insensitively, in first-seen order.
pub fn org_logins(repos: &[Value], user_orgs: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let from_repos = repos
        .iter()
        .filter(|r| s(r, "/owner/type").as_deref() == Some("Organization"))
        .filter_map(|r| s(r, "/owner/login"));
    let from_orgs = user_orgs.iter().filter_map(|o| s(o, "/login"));
    for login in from_repos.chain(from_orgs) {
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&login)) {
            out.push(login);
        }
    }
    out
}

/// One query fetching several orgs' projects; logins go in as variables (never
/// interpolated), each org under alias `o<i>`.
fn org_projects_query(logins: &[String]) -> (String, Value) {
    let decls: Vec<String> = (0..logins.len()).map(|i| format!("$o{i}: String!")).collect();
    let body: Vec<String> = (0..logins.len())
        .map(|i| format!("o{i}: organization(login: $o{i}) {{ projectsV2(first: 50) {{ nodes {{ {PROJECT_FIELDS} }} }} }}"))
        .collect();
    let vars: serde_json::Map<String, Value> = logins
        .iter()
        .enumerate()
        .map(|(i, l)| (format!("o{i}"), Value::String(l.clone())))
        .collect();
    (
        format!("query({}) {{ {} }}", decls.join(", "), body.join(" ")),
        Value::Object(vars),
    )
}

/// Open projects from an aliased `org_projects_query` response (null orgs skipped).
pub fn parse_org_projects(data: &Value) -> Vec<GhProjectInfo> {
    let Some(obj) = data.as_object() else { return Vec::new() };
    let mut wrapped = json!({ "viewer": { "organizations": { "nodes": [] } } });
    if let Some(nodes) = wrapped.pointer_mut("/viewer/organizations/nodes").and_then(Value::as_array_mut) {
        nodes.extend(obj.values().cloned());
    }
    parse_projects(&wrapped).projects
}

/// The board's "status" column: the single-select field named Status, else the first
/// single-select field. Returns (field name, option order).
pub fn pick_status_field(project: &Value) -> (String, Vec<String>) {
    let fields: Vec<(String, Vec<String>)> = project
        .pointer("/fields/nodes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let name = f.get("name")?.as_str()?.to_string();
                    let opts = f
                        .get("options")?
                        .as_array()?
                        .iter()
                        .filter_map(|o| o.get("name").and_then(Value::as_str).map(str::to_string))
                        .collect();
                    Some((name, opts))
                })
                .collect()
        })
        .unwrap_or_default();
    fields
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("status"))
        .or_else(|| fields.first())
        .cloned()
        .unwrap_or_default()
}

pub fn parse_project_item(n: &Value, source_id: &str, status_field: &str) -> Option<GhItem> {
    let id = s(n, "/id")?;
    let ty = s(n, "/type").unwrap_or_default();
    let kind = match ty.as_str() {
        "ISSUE" => "issue",
        "PULL_REQUEST" => "pr",
        "DRAFT_ISSUE" => "draft",
        _ => return None, // REDACTED (no access) and future types
    };
    let c = n.get("content").filter(|c| !c.is_null())?;

    let mut status = None;
    let mut fields = Vec::new();
    if let Some(vals) = n.pointer("/fieldValues/nodes").and_then(Value::as_array) {
        for fv in vals {
            let Some(fname) = s(fv, "/field/name") else { continue };
            let value = s(fv, "/name")
                .or_else(|| s(fv, "/title"))
                .or_else(|| s(fv, "/text"))
                .or_else(|| s(fv, "/date"))
                .or_else(|| fv.get("number").and_then(Value::as_f64).map(|x| x.to_string()));
            let Some(value) = value else { continue };
            if !status_field.is_empty() && fname == status_field {
                status = Some(value);
            } else if fname != "Title" {
                fields.push(GhField { name: fname, value });
            }
        }
    }

    let item_key = match (s(c, "/repository/nameWithOwner"), c.get("number").and_then(Value::as_i64)) {
        (Some(r), Some(num)) => format!("{r}#{num}"),
        _ => format!("draft:{id}"),
    };
    Some(content_to_item(c, source_id, kind, item_key, status, fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "github.com
  ✓ Logged in to github.com account alice (keyring)
  - Active account: true
  - Git operations protocol: https
  - Token: gho_************************************
  - Token scopes: 'gist', 'read:org', 'repo', 'workflow'

  ✓ Logged in to github.com account bob-work (keyring)
  - Active account: false
  - Git operations protocol: https
  - Token: gho_************************************
  - Token scopes: 'project', 'repo'
ghe.example.com
  ✓ Logged in to ghe.example.com account carol (keyring)
  - Active account: true
";

    #[test]
    fn parses_gh_auth_status() {
        let a = parse_auth_status(STATUS);
        assert_eq!(a.len(), 2, "other hosts are ignored");
        assert_eq!(a[0].login, "alice");
        assert!(a[0].active);
        assert!(!scopes_allow_projects(a[0].scopes.as_deref().unwrap()));
        assert_eq!(a[1].login, "bob-work");
        assert!(!a[1].active);
        assert!(scopes_allow_projects(a[1].scopes.as_deref().unwrap()));
    }

    #[test]
    fn resolves_gh_from_candidates_then_path() {
        let dir = std::env::temp_dir().join(format!("gh-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("gh");
        std::fs::write(&fake, "").unwrap();
        assert_eq!(resolve_gh(&["/nonexistent/gh"], Some(dir.as_os_str())), Some(fake.clone()));
        assert_eq!(resolve_gh(&[fake.to_str().unwrap()], None), Some(fake.clone()));
        assert_eq!(resolve_gh(&["/nonexistent/gh"], None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn iso_and_links() {
        assert_eq!(iso_to_ms("1970-01-01T00:00:00Z"), 0);
        assert_eq!(iso_to_ms("2024-02-29T12:00:00Z"), 1_709_208_000_000);
        assert_eq!(iso_to_ms("junk"), 0);
        let link = r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=5>; rel="last""#;
        assert_eq!(next_link(Some(link)).as_deref(), Some("https://api.github.com/x?page=2"));
        assert_eq!(next_link(Some(r#"<https://a/1>; rel="prev""#)), None);
    }

    #[test]
    fn parses_content_with_comment_and_edit_times() {
        let c = json!({
            "number": 7, "title": "Bug", "state": "OPEN", "url": "https://github.com/o/r/issues/7",
            "author": {"login": "alice"}, "assignees": {"nodes": [{"login": "bob"}]},
            "labels": {"nodes": [{"name": "bug", "color": "d73a4a"}, {"name": "evil", "color": "red;x:url(y)"}]},
            "comments": {"totalCount": 3, "nodes": [{"createdAt": "1970-01-01T00:00:05Z", "author": {"login": "carol"}}]},
            "createdAt": "1970-01-01T00:00:01Z", "updatedAt": "1970-01-01T00:00:09Z", "lastEditedAt": null,
            "repository": {"nameWithOwner": "o/r"}, "body": "<script>"
        });
        let i = content_to_item(&c, "src", "issue", "o/r#7".into(), None, vec![]);
        assert_eq!(i.state, "open");
        assert_eq!(i.number, Some(7));
        assert_eq!((i.created_at, i.updated_at), (1_000, 9_000));
        assert_eq!(i.edited_at, None);
        assert_eq!(i.last_comment_at, Some(5_000));
        assert_eq!(i.last_comment_by.as_deref(), Some("carol"));
        assert_eq!(i.comments, 3);
        assert_eq!(i.labels[0].color.as_deref(), Some("#d73a4a"));
        assert_eq!(i.labels[1].color, None, "non-hex colors are dropped");

        // Old cached JSON (before the new fields) still deserializes.
        let old = r#"{"sourceId":"s","itemKey":"k","kind":"issue","repo":null,"number":null,"title":"t",
            "state":"open","url":null,"author":null,"assignees":[],"labels":[],"comments":0,
            "updatedAt":1,"status":null,"fields":[],"body":""}"#;
        let parsed: GhItem = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.last_comment_at, None);
    }

    #[test]
    fn parses_project_items_and_status() {
        let project = json!({
            "fields": {"nodes": [
                {}, {"name": "Priority", "options": [{"name": "P0"}]},
                {"name": "Status", "options": [{"name": "Todo"}, {"name": "In Progress"}, {"name": "Done"}]}
            ]}
        });
        let (field, opts) = pick_status_field(&project);
        assert_eq!(field, "Status");
        assert_eq!(opts, vec!["Todo", "In Progress", "Done"]);
        // No "Status" → first single-select.
        let (f2, _) = pick_status_field(&json!({"fields": {"nodes": [{"name": "Stage", "options": []}]}}));
        assert_eq!(f2, "Stage");

        let issue = json!({
            "id": "I1", "type": "ISSUE",
            "fieldValues": {"nodes": [
                {"text": "Bug", "field": {"name": "Title"}},
                {"name": "In Progress", "field": {"name": "Status"}},
                {"name": "P0", "field": {"name": "Priority"}},
                {"title": "Sprint 3", "field": {"name": "Iteration"}},
                {}
            ]},
            "content": {"number": 4, "title": "Bug", "url": "u", "state": "OPEN",
                        "repository": {"nameWithOwner": "o/api"}, "comments": {"totalCount": 2}}
        });
        let i = parse_project_item(&issue, "p", "Status").unwrap();
        assert_eq!(i.status.as_deref(), Some("In Progress"));
        assert_eq!(i.item_key, "o/api#4");
        assert_eq!(i.state, "open");
        assert_eq!(i.fields.len(), 2, "Title is skipped, Priority + Iteration kept");

        let draft = json!({"id": "D1", "type": "DRAFT_ISSUE", "fieldValues": {"nodes": []},
                           "content": {"title": "Idea", "body": ""}});
        let d = parse_project_item(&draft, "p", "Status").unwrap();
        assert_eq!(d.kind, "draft");
        assert_eq!(d.item_key, "draft:D1");
        assert!(d.url.is_none() && d.number.is_none() && d.status.is_none());

        let redacted = json!({"id": "R1", "type": "REDACTED", "content": null});
        assert!(parse_project_item(&redacted, "p", "Status").is_none());
    }

    #[test]
    fn discovers_orgs_from_repo_owners() {
        let repos = vec![
            json!({"owner": {"login": "Visist-ai", "type": "Organization"}}),
            json!({"owner": {"login": "visist-AI", "type": "Organization"}}),
            json!({"owner": {"login": "someone", "type": "User"}}),
        ];
        let orgs = vec![json!({"login": "acme"}), json!({"login": "Visist-ai"})];
        assert_eq!(org_logins(&repos, &orgs), vec!["Visist-ai", "acme"]);

        let (q, vars) = org_projects_query(&["a".into(), "b\"} evil".into()]);
        assert!(q.starts_with("query($o0: String!, $o1: String!)"));
        assert!(!q.contains("evil"), "logins are variables, never interpolated");
        assert_eq!(vars["o1"], "b\"} evil");

        let data = json!({
            "o0": {"projectsV2": {"nodes": [{"id": "P9", "title": "Visist-Dev", "number": 9, "url": "u", "closed": false, "owner": {"login": "Visist-ai"}}]}},
            "o1": null
        });
        let found = parse_org_projects(&data);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].number, 9);
    }

    #[test]
    fn graphql_partial_errors_keep_data() {
        let v = json!({
            "data": {"viewer": {
                "projectsV2": {"nodes": [{"id": "P1", "title": "Mine", "number": 1, "url": "u", "closed": false, "owner": {"login": "alice"}}]},
                "organizations": {"nodes": [
                    null,
                    {"login": "acme", "projectsV2": {"nodes": [
                        {"id": "P2", "title": "Roadmap", "number": 2, "url": "u2", "closed": false, "owner": {"login": "acme"}},
                        {"id": "P3", "title": "Old", "number": 3, "url": "u3", "closed": true, "owner": {"login": "acme"}}
                    ]}}
                ]}
            }},
            "errors": [{"type": "FORBIDDEN", "message": "SAML enforcement for corp"}]
        });
        let (data, errors) = split_graphql(v);
        assert_eq!(errors.len(), 1);
        let list = parse_projects(&data);
        let titles: Vec<_> = list.projects.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["Mine", "Roadmap"], "closed projects skipped");
    }

    /// Live, read-only smoke test against the gh accounts on this machine:
    /// `cargo test --lib live_read_only -- --ignored --nocapture`. Prints counts only.
    #[test]
    #[ignore]
    fn live_read_only() {
        for a in gh_cli_accounts() {
            if a.scopes.as_deref().is_some_and(scopes_allow_projects) {
                // GraphQL validates before executing: a bogus id must fail with
                // NOT_FOUND, not a schema/validation error.
                let c = Client::new(gh_token(&a.login).expect("token"));
                println!("{} schema check: {:?}", a.login, c.project_items("t", "PVT_doesnotexist").err());
            }
            let client = Client::new(gh_token(&a.login).expect("token"));
            let (login, _) = client.whoami().expect("whoami");
            let repos = client.list_repos().map(|r| r.len());
            let projects = client.list_projects();
            println!(
                "{login}: repos={repos:?} projects={:?}",
                projects.as_ref().map(|p| (p.projects.len(), p.notices.len()))
            );
            if let Ok(r) = client.list_repos() {
                if let Some(first) = r.first() {
                    let items = client.repo_items("t", &first.full_name);
                    let commented = items.as_ref().map(|i| i.iter().filter(|x| x.last_comment_at.is_some()).count());
                    println!(
                        "  {} items={:?} with_last_comment={commented:?}",
                        first.full_name,
                        items.as_ref().map(|i| i.len())
                    );
                }
            }
            if let Ok(p) = projects {
                if let Some(first) = p.projects.first() {
                    let items = client
                        .project_items("t", &first.id)
                        .map(|(i, o)| (i.len(), i.iter().filter(|x| x.last_comment_at.is_some()).count(), o));
                    println!("  project {} items={items:?}", first.title);
                }
            }
        }
    }
}
