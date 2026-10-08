// GitHub Issues pane. First run: pick an account (from the gh CLI, or paste a token),
// then pick repos and/or Projects v2 boards. Items are cached locally (SQLite) so the
// pane opens instantly, and refreshed every few minutes.
//
// Security: issue content is untrusted. Titles/bodies/labels render as plain text only
// (no HTML, no markdown); label colors arrive pre-validated from the backend; links are
// opened only when they point at https://github.com/.

import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  createTask,
  ghAccounts,
  ghAddToken,
  ghGetAccount,
  ghItems,
  ghListProjects,
  ghListRepos,
  ghSetAccount,
  ghSetSources,
  ghSources,
  ghSync,
  gitRemotes,
} from "../ipc/api";
import type {
  GhAccount,
  GhItem,
  GhProjectInfo,
  GhRepoInfo,
  GhSource,
  GhSourceInput,
} from "../ipc/types";
import { inkOn } from "../lib/projectColors";
import {
  IconBoard,
  IconClock,
  IconComment,
  IconExternal,
  IconIssue,
  IconIssueClosed,
  IconIssueDraft,
  IconListPlus,
  IconMerged,
  IconPullRequest,
  IconRefresh,
  IconRepo,
  IconSearch,
  IconSettings,
} from "./Icons";

interface IssuesPanelProps {
  /** Active terminal's cwd — used to auto-filter to the repo you're working in. */
  cwd: string | null;
}

const SYNC_EVERY_MS = 5 * 60_000;
const TICK_MS = 60_000;
const BODY_PREVIEW = 1200;
const SHOW_PRS_KEY = "conductor-issues-show-prs";
const SORT_KEY = "conductor-issues-sort";

type View = "loading" | "accounts" | "sources" | "list";
type Who = "all" | "assigned" | "created";
type SortBy = "updated" | "commented" | "created" | "edited";

const SORT_LABEL: Record<SortBy, string> = {
  updated: "Recently updated",
  commented: "Last comment",
  created: "Newest",
  edited: "Recently edited",
};

/** Timestamp an item sorts by; items lacking it (no comments / never edited) sink. */
function sortValue(i: GhItem, by: SortBy): number {
  switch (by) {
    case "commented":
      return i.lastCommentAt ?? 0;
    case "created":
      return i.createdAt;
    case "edited":
      return i.editedAt ?? 0;
    default:
      return i.updatedAt;
  }
}
/** "auto" = the active terminal's repo when it has items, else everything. */
type SourceFilter = "auto" | "all" | string;

function fmtAgo(ms: number, now: number): string {
  const m = Math.max(0, Math.floor((now - ms) / 60_000));
  if (m < 1) return "just now";
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return d < 30 ? `${d}d ago` : new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function safeOpen(url: string | null) {
  if (url && url.startsWith("https://github.com/")) void openUrl(url);
}

/** "https://github.com/owner/name" → "owner/name". */
function repoFromWebUrl(u: string | null): string | null {
  const m = u ? /^https:\/\/github\.com\/([^/]+\/[^/]+?)\/?$/.exec(u) : null;
  return m ? m[1].toLowerCase() : null;
}

function shortRepo(repo: string | null): string {
  return repo ? repo.split("/")[1] ?? repo : "";
}

export function IssuesPanel({ cwd }: IssuesPanelProps) {
  const [view, setView] = useState<View>("loading");
  const [accounts, setAccounts] = useState<GhAccount[]>([]);
  const [account, setAccount] = useState<string | null>(null);
  const [sources, setSources] = useState<GhSource[]>([]);
  const [items, setItems] = useState<GhItem[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [currentRepo, setCurrentRepo] = useState<string | null>(null);

  const [sourceFilter, setSourceFilter] = useState<SourceFilter>("auto");
  const [who, setWho] = useState<Who>("all");
  const [query, setQuery] = useState("");
  const [showPrs, setShowPrs] = useState(() => {
    try {
      return localStorage.getItem(SHOW_PRS_KEY) !== "0";
    } catch {
      return true;
    }
  });
  const [expanded, setExpanded] = useState<string | null>(null);
  const [sortBy, setSortBy] = useState<SortBy>(() => {
    try {
      const v = localStorage.getItem(SORT_KEY);
      return v === "commented" || v === "created" || v === "edited" ? v : "updated";
    } catch {
      return "updated";
    }
  });

  useEffect(() => {
    const iv = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(iv);
  }, []);

  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), 2500);
    return () => clearTimeout(t);
  }, [notice]);

  // Which GitHub repo is the active terminal in?
  useEffect(() => {
    let active = true;
    if (!cwd) {
      setCurrentRepo(null);
      return;
    }
    gitRemotes(cwd)
      .then((rs) => {
        if (!active) return;
        const origin = rs.find((r) => r.name === "origin") ?? rs[0];
        setCurrentRepo(repoFromWebUrl(origin?.webUrl ?? null));
      })
      .catch(() => active && setCurrentRepo(null));
    return () => {
      active = false;
    };
  }, [cwd]);

  const loadCache = useCallback(async (acct: string) => {
    const [src, its] = await Promise.all([ghSources(acct), ghItems(acct)]);
    setSources(src);
    setItems(its);
    return src;
  }, []);

  const sync = useCallback(
    async (acct: string) => {
      setSyncing(true);
      try {
        const src = await ghSync(acct);
        setSources(src);
        setItems(await ghItems(acct));
        setError(null);
      } catch (e) {
        setError(String(e));
      } finally {
        setSyncing(false);
      }
    },
    [],
  );

  // Boot: a saved account goes straight to the cached list (works offline — no
  // dependency on `gh auth status`); accounts load in the background for the picker.
  useEffect(() => {
    let active = true;
    ghAccounts()
      .then((a) => active && setAccounts(a))
      .catch(() => {});
    (async () => {
      try {
        const saved = await ghGetAccount();
        if (!active) return;
        if (!saved) {
          setView("accounts");
          return;
        }
        setAccount(saved);
        const src = await loadCache(saved);
        if (!active) return;
        if (src.length === 0) {
          setView("sources");
          return;
        }
        setView("list");
        const oldest = Math.min(...src.map((s) => s.syncedAt ?? 0));
        if (Date.now() - oldest > SYNC_EVERY_MS) void sync(saved);
      } catch (e) {
        if (active) {
          setError(String(e));
          setView("accounts");
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [loadCache, sync]);

  // Periodic refresh while the list is showing.
  useEffect(() => {
    if (view !== "list" || !account) return;
    const iv = setInterval(() => void sync(account), SYNC_EVERY_MS);
    return () => clearInterval(iv);
  }, [view, account, sync]);

  const chooseAccount = useCallback(
    async (login: string) => {
      await ghSetAccount(login);
      setAccount(login);
      setSourceFilter("auto");
      const src = await loadCache(login);
      setView(src.length ? "list" : "sources");
      if (src.length) void sync(login);
    },
    [loadCache, sync],
  );

  const saveSources = useCallback(
    async (picked: GhSourceInput[]) => {
      if (!account) return;
      try {
        setSources(await ghSetSources(account, picked));
        setItems(await ghItems(account));
        setView("list");
        if (picked.length) void sync(account);
      } catch (e) {
        setError(String(e));
      }
    },
    [account, sync],
  );

  const toTask = useCallback(async (it: GhItem) => {
    const ref = it.number != null ? `#${it.number} ` : "";
    try {
      await createTask({
        id: "",
        title: `${ref}${it.title}`,
        notes: it.url ?? "",
        status: "todo",
        priority: 2,
        project: it.repo ? shortRepo(it.repo) : null,
        dueAt: null,
        createdAt: 0,
        updatedAt: 0,
        startedAt: null,
        completedAt: null,
      });
      setNotice("Added to Tasks");
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // ---- derived list ----
  const me = account?.toLowerCase() ?? "";
  const sourceById = useMemo(() => new Map(sources.map((s) => [s.id, s])), [sources]);
  const currentRepoHasItems =
    !!currentRepo && items.some((i) => i.repo?.toLowerCase() === currentRepo);
  const effectiveFilter: SourceFilter =
    sourceFilter === "auto" ? (currentRepoHasItems ? "auto" : "all") : sourceFilter;
  const activeProject =
    effectiveFilter !== "auto" && effectiveFilter !== "all"
      ? sourceById.get(effectiveFilter)
      : undefined;

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    let list = items.filter((i) => {
      if (effectiveFilter === "auto") {
        if (i.repo?.toLowerCase() !== currentRepo) return false;
      } else if (effectiveFilter !== "all" && i.sourceId !== effectiveFilter) {
        return false;
      }
      if (!showPrs && i.kind === "pr") return false;
      if (who === "assigned" && !i.assignees.some((a) => a.toLowerCase() === me)) return false;
      if (who === "created" && i.author?.toLowerCase() !== me) return false;
      if (q) {
        const hay = `${i.title} ${i.repo ?? ""} ${i.number ?? ""} ${i.labels.map((l) => l.name).join(" ")}`;
        if (!hay.toLowerCase().includes(q)) return false;
      }
      return true;
    });
    // The same issue can come from a repo and a project: keep one, preferring the copy
    // that carries a project status.
    if (!activeProject) {
      const byKey = new Map<string, GhItem>();
      for (const i of list) {
        const prev = byKey.get(i.itemKey);
        if (!prev || (!prev.status && i.status)) byKey.set(i.itemKey, i);
      }
      list = [...byKey.values()];
    }
    return list.sort((a, b) => sortValue(b, sortBy) - sortValue(a, sortBy) || b.updatedAt - a.updatedAt);
  }, [items, effectiveFilter, currentRepo, showPrs, who, me, query, activeProject, sortBy]);

  /** Projects group by their status column (in board order); everything else is flat. */
  const groups = useMemo((): { name: string | null; items: GhItem[] }[] => {
    if (!activeProject || activeProject.kind !== "project") return [{ name: null, items: visible }];
    const order = activeProject.statusOptions;
    const map = new Map<string, GhItem[]>();
    for (const i of visible) {
      const k = i.status ?? "No status";
      map.set(k, [...(map.get(k) ?? []), i]);
    }
    const names = [...order.filter((o) => map.has(o)), ...[...map.keys()].filter((k) => !order.includes(k))];
    return names.map((n) => ({ name: n, items: map.get(n) ?? [] }));
  }, [visible, activeProject]);

  const lastSync = sources.reduce<number | null>(
    (acc, s) => (s.syncedAt && (!acc || s.syncedAt > acc) ? s.syncedAt : acc),
    null,
  );
  const sourceErrors = sources.filter((s) => s.error);

  // ---- render ----
  if (view === "loading") {
    return (
      <div className="iss-panel">
        <div className="iss-empty">Loading GitHub accounts…</div>
      </div>
    );
  }

  if (view === "accounts" || !account) {
    return (
      <div className="iss-panel">
        <AccountPicker
          accounts={accounts}
          current={account}
          error={error}
          onPick={(l) => void chooseAccount(l).catch((e) => setError(String(e)))}
          onAdded={(a) => {
            setAccounts((prev) => [...prev.filter((x) => x.login !== a.login), a]);
            void chooseAccount(a.login).catch((e) => setError(String(e)));
          }}
          onCancel={account ? () => setView("list") : undefined}
        />
      </div>
    );
  }

  if (view === "sources") {
    return (
      <div className="iss-panel">
        <SourcePicker
          account={accounts.find((a) => a.login === account) ?? null}
          login={account}
          selected={sources}
          onSave={(p) => void saveSources(p)}
          onCancel={sources.length ? () => setView("list") : undefined}
          onSwitchAccount={() => setView("accounts")}
        />
      </div>
    );
  }

  return (
    <div className="iss-panel">
      <div className="iss-head">
        <button className="iss-account" onClick={() => setView("accounts")} title="Switch account">
          <span className="iss-avatar iss-avatar--sm">{account.slice(0, 1).toUpperCase()}</span>
          {account}
        </button>
        <span className="iss-sync">{syncing ? "Syncing…" : lastSync ? `Synced ${fmtAgo(lastSync, now)}` : ""}</span>
        <button className="iss-icon" onClick={() => void sync(account)} disabled={syncing} title="Refresh">
          <IconRefresh size={13} className={syncing ? "iss-spin" : ""} />
        </button>
        <button className="iss-icon" onClick={() => setView("sources")} title="Choose repos & projects">
          <IconSettings size={13} />
        </button>
      </div>

      <div className="iss-controls">
        <div className="iss-chips">
          {currentRepoHasItems && (
            <button
              className={`chip ${effectiveFilter === "auto" ? "chip--on" : ""}`}
              onClick={() => setSourceFilter("auto")}
              title="Repo of the active terminal"
            >
              <IconRepo size={11} />
              {shortRepo(currentRepo)} · here
            </button>
          )}
          <button
            className={`chip ${effectiveFilter === "all" ? "chip--on" : ""}`}
            onClick={() => setSourceFilter("all")}
          >
            All
          </button>
          {sources.map((s) => (
            <button
              key={s.id}
              className={`chip ${effectiveFilter === s.id ? "chip--on" : ""}`}
              onClick={() => setSourceFilter(s.id)}
              title={s.kind === "project" ? `Project · ${s.title}` : s.key}
            >
              {s.kind === "project" ? <IconBoard size={11} /> : <IconRepo size={11} />}
              {s.kind === "project" ? s.title : shortRepo(s.key)}
            </button>
          ))}
        </div>
        <div className="iss-filter-row">
          <div className="iss-seg">
            {(["all", "assigned", "created"] as Who[]).map((w) => (
              <button key={w} className={who === w ? "iss-seg--on" : ""} onClick={() => setWho(w)}>
                {w === "all" ? "All" : w === "assigned" ? "Assigned" : "Mine"}
              </button>
            ))}
          </div>
          <select
            className="iss-sort"
            value={sortBy}
            title="Sort"
            onChange={(e) => {
              const v = e.target.value as SortBy;
              setSortBy(v);
              try {
                localStorage.setItem(SORT_KEY, v);
              } catch {
                /* ignore */
              }
            }}
          >
            {(Object.keys(SORT_LABEL) as SortBy[]).map((k) => (
              <option key={k} value={k}>
                {SORT_LABEL[k]}
              </option>
            ))}
          </select>
          <label className="iss-pr-toggle" title="Show pull requests">
            <input
              type="checkbox"
              checked={showPrs}
              onChange={(e) => {
                setShowPrs(e.target.checked);
                try {
                  localStorage.setItem(SHOW_PRS_KEY, e.target.checked ? "1" : "0");
                } catch {
                  /* ignore */
                }
              }}
            />
            PRs
          </label>
        </div>
        <div className="iss-search">
          <IconSearch size={12} />
          <input
            placeholder="Filter by title, #, label…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        {error && <div className="task-error">{error}</div>}
        {sourceErrors.map((s) => (
          <div key={s.id} className="task-error" title={s.error ?? ""}>
            {s.kind === "project" ? s.title : s.key}: {s.error}
          </div>
        ))}
        {notice && <div className="iss-notice">{notice}</div>}
      </div>

      <div className="task-list iss-list">
        {visible.length === 0 && (
          <div className="iss-empty">
            {syncing ? "Fetching…" : items.length === 0 ? "No items yet — press ↻ to sync." : "Nothing matches."}
          </div>
        )}
        {groups.map((g) => (
          <section key={g.name ?? "_"} className="task-section">
            {g.name && (
              <div className="task-section-h iss-group-h">
                <span className="iss-group-dot" />
                {g.name}
                <span className="task-count">{g.items.length}</span>
              </div>
            )}
            {g.items.map((it) => {
              const key = `${it.sourceId}/${it.itemKey}`;
              return (
                <IssueRow
                  key={key}
                  item={it}
                  now={now}
                  source={sourceById.get(it.sourceId)}
                  showStatus={!g.name}
                  sortBy={sortBy}
                  open={expanded === key}
                  onToggle={() => setExpanded((e) => (e === key ? null : key))}
                  onTask={() => void toTask(it)}
                />
              );
            })}
          </section>
        ))}
      </div>
    </div>
  );
}

// ---- rows -------------------------------------------------------------------

interface IssueRowProps {
  item: GhItem;
  now: number;
  source: GhSource | undefined;
  showStatus: boolean;
  sortBy: SortBy;
  open: boolean;
  onToggle: () => void;
  onTask: () => void;
}

/** Icon + tone for an item's kind/state, GitHub-style. */
function kindIcon(it: GhItem): { icon: ReactNode; tone: string; label: string } {
  if (it.kind === "draft") return { icon: <IconIssueDraft size={14} />, tone: "draft", label: "Draft" };
  if (it.kind === "pr") {
    if (it.state === "merged") return { icon: <IconMerged size={14} />, tone: "merged", label: "Merged PR" };
    if (it.state === "closed") return { icon: <IconPullRequest size={14} />, tone: "closed", label: "Closed PR" };
    return { icon: <IconPullRequest size={14} />, tone: "pr", label: "Open PR" };
  }
  if (it.state === "closed") return { icon: <IconIssueClosed size={14} />, tone: "done", label: "Closed issue" };
  return { icon: <IconIssue size={14} />, tone: "open", label: "Open issue" };
}

/** The time shown on an item follows the active sort. */
function whenFor(it: GhItem, by: SortBy, now: number): { icon: ReactNode; text: string; title: string } {
  const clock = <IconClock size={10} />;
  if (by === "commented") {
    return it.lastCommentAt
      ? {
          icon: <IconComment size={10} />,
          text: fmtAgo(it.lastCommentAt, now),
          title: `Last comment ${new Date(it.lastCommentAt).toLocaleString()}${it.lastCommentBy ? ` by @${it.lastCommentBy}` : ""}`,
        }
      : { icon: <IconComment size={10} />, text: "no comments", title: "No comments yet" };
  }
  if (by === "created" && it.createdAt) {
    return { icon: clock, text: `opened ${fmtAgo(it.createdAt, now)}`, title: new Date(it.createdAt).toLocaleString() };
  }
  if (by === "edited") {
    return it.editedAt
      ? { icon: clock, text: `edited ${fmtAgo(it.editedAt, now)}`, title: new Date(it.editedAt).toLocaleString() }
      : { icon: clock, text: "never edited", title: "Title/description never edited" };
  }
  return { icon: clock, text: fmtAgo(it.updatedAt, now), title: `Updated ${new Date(it.updatedAt).toLocaleString()}` };
}

function IssueRow({ item: it, now, source, showStatus, sortBy, open, onToggle, onTask }: IssueRowProps) {
  const when = whenFor(it, sortBy, now);
  const [full, setFull] = useState(false);
  const body = it.body.trim();
  const shown = full || body.length <= BODY_PREVIEW ? body : `${body.slice(0, BODY_PREVIEW)}…`;
  const k = kindIcon(it);
  const extraLabels = it.labels.length - 3;
  return (
    <div className={`iss-card iss-card--${k.tone} ${open ? "iss-card--open" : ""}`}>
      <button className="iss-card-main" onClick={onToggle}>
        <span className="iss-card-top">
          <span className={`iss-kind iss-kind--${k.tone}`} title={k.label}>
            {k.icon}
          </span>
          <span className="iss-ref">
            {it.kind === "draft" ? "Draft" : `${shortRepo(it.repo)} #${it.number ?? ""}`}
          </span>
          {showStatus && it.status && <span className="iss-status">{it.status}</span>}
          <span className="iss-when" title={when.title}>
            {when.icon}
            {when.text}
          </span>
        </span>
        <span className="iss-title">{it.title || "(untitled)"}</span>
        {(it.labels.length > 0 || it.assignees.length > 0 || it.comments > 0) && (
          <span className="iss-card-foot">
            <span className="iss-labels">
              {it.labels.slice(0, 3).map((l) => (
                <span
                  key={l.name}
                  className="iss-label"
                  style={l.color ? { background: l.color, color: inkOn(l.color) } : undefined}
                >
                  {l.name}
                </span>
              ))}
              {extraLabels > 0 && <span className="iss-label iss-label--more">+{extraLabels}</span>}
            </span>
            {it.comments > 0 && (
              <span
                className="iss-comments"
                title={
                  it.lastCommentAt
                    ? `${it.comments} comments · last ${fmtAgo(it.lastCommentAt, now)}${it.lastCommentBy ? ` by @${it.lastCommentBy}` : ""}`
                    : `${it.comments} comments`
                }
              >
                <IconComment size={11} />
                {it.comments}
              </span>
            )}
            {it.assignees.length > 0 && (
              <span className="iss-assignees" title={`Assigned: ${it.assignees.join(", ")}`}>
                {it.assignees.slice(0, 3).map((a) => (
                  <span key={a} className="iss-avatar iss-avatar--xs">
                    {a.slice(0, 1).toUpperCase()}
                  </span>
                ))}
              </span>
            )}
          </span>
        )}
      </button>
      {open && (
        <div className="iss-card-detail">
          {(it.fields.length > 0 || source || it.author || it.createdAt > 0) && (
            <div className="iss-facts">
              {source && (
                <span className="iss-fact">
                  {source.kind === "project" ? <IconBoard size={11} /> : <IconRepo size={11} />}
                  {source.kind === "project" ? source.title : source.key}
                </span>
              )}
              {it.author && <span className="iss-fact">by @{it.author}</span>}
              {it.createdAt > 0 && <span className="iss-fact">opened {fmtAgo(it.createdAt, now)}</span>}
              {it.editedAt && <span className="iss-fact">edited {fmtAgo(it.editedAt, now)}</span>}
              {it.lastCommentAt && (
                <span className="iss-fact">
                  <IconComment size={10} />
                  {fmtAgo(it.lastCommentAt, now)}
                  {it.lastCommentBy && ` · @${it.lastCommentBy}`}
                </span>
              )}
              {it.fields.map((f) => (
                <span key={f.name} className="iss-fact">
                  <b>{f.name}</b> {f.value}
                </span>
              ))}
            </div>
          )}
          {body ? (
            <div className="iss-body">
              {shown}
              {body.length > BODY_PREVIEW && (
                <button className="iss-more" onClick={() => setFull((v) => !v)}>
                  {full ? "Show less" : "Show more"}
                </button>
              )}
            </div>
          ) : (
            <div className="iss-nobody">No description.</div>
          )}
          <div className="iss-actions">
            <button className="iss-action" onClick={onTask} title="Add to Tasks">
              <IconListPlus size={12} />
              Add to Tasks
            </button>
            {it.url && (
              <button className="iss-action iss-action--primary" onClick={() => safeOpen(it.url)}>
                <IconExternal size={12} />
                Open on GitHub
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ---- setup: account ----------------------------------------------------------

interface AccountPickerProps {
  accounts: GhAccount[];
  current: string | null;
  error: string | null;
  onPick: (login: string) => void;
  onAdded: (a: GhAccount) => void;
  onCancel?: () => void;
}

function AccountPicker({ accounts, current, error, onPick, onAdded, onCancel }: AccountPickerProps) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const add = async () => {
    setBusy(true);
    setErr(null);
    try {
      onAdded(await ghAddToken(token));
      setToken("");
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="iss-setup">
      <div className="iss-setup-title">Choose a GitHub account</div>
      {accounts.length === 0 ? (
        <p className="iss-hint">
          No GitHub CLI accounts found. Install <code>gh</code> and run <code>gh auth login</code>, or paste a
          token below.
        </p>
      ) : (
        <div className="iss-accounts">
          {accounts.map((a) => (
            <button
              key={a.login}
              className={`iss-account-row ${a.login === current ? "iss-account-row--on" : ""}`}
              onClick={() => onPick(a.login)}
            >
              <span className="iss-avatar">{a.login.slice(0, 1).toUpperCase()}</span>
              <span className="iss-account-name">
                @{a.login}
                <span className="iss-hint">
                  {a.via === "gh" ? "GitHub CLI" : "Saved token"} ·{" "}
                  {a.canProjects ? "repos + projects" : "repos only (no project scope)"}
                </span>
              </span>
            </button>
          ))}
        </div>
      )}

      <div className="iss-setup-sub">Or use a personal access token</div>
      <input
        className="task-add-input"
        type="password"
        placeholder="ghp_… or github_pat_…"
        value={token}
        onChange={(e) => setToken(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && token.trim() && void add()}
      />
      <p className="iss-hint">Needs repo + read:project scopes. Stored in the macOS Keychain, never in the app's database.</p>
      <div className="task-detail-foot">
        {onCancel ? (
          <button className="btn btn-sm" onClick={onCancel}>
            Cancel
          </button>
        ) : (
          <span />
        )}
        <button className="btn btn-sm btn-primary" disabled={!token.trim() || busy} onClick={() => void add()}>
          {busy ? "Checking…" : "Add token"}
        </button>
      </div>
      {(err || error) && <div className="task-error">{err ?? error}</div>}
    </div>
  );
}

// ---- setup: repos & projects ---------------------------------------------------

interface SourcePickerProps {
  account: GhAccount | null;
  login: string;
  selected: GhSource[];
  onSave: (picked: GhSourceInput[]) => void;
  onCancel?: () => void;
  onSwitchAccount: () => void;
}

function SourcePicker({ account, login, selected, onSave, onCancel, onSwitchAccount }: SourcePickerProps) {
  const [tab, setTab] = useState<"repos" | "projects">("repos");
  const [repos, setRepos] = useState<GhRepoInfo[] | null>(null);
  const [projects, setProjects] = useState<GhProjectInfo[] | null>(null);
  const [repoErr, setRepoErr] = useState<string | null>(null);
  const [projErr, setProjErr] = useState<string | null>(null);
  const [notices, setNotices] = useState<string[]>([]);
  const [q, setQ] = useState("");
  const [picked, setPicked] = useState<Map<string, GhSourceInput>>(
    () => new Map(selected.map((s) => [`${s.kind}|${s.key}`, { kind: s.kind, key: s.key, title: s.title, url: s.url }])),
  );

  useEffect(() => {
    let active = true;
    ghListRepos(login)
      .then((r) => active && setRepos(r))
      .catch((e) => active && setRepoErr(String(e)));
    ghListProjects(login)
      .then((p) => {
        if (!active) return;
        setProjects(p.projects);
        setNotices(p.notices);
      })
      .catch((e) => active && setProjErr(String(e)));
    return () => {
      active = false;
    };
  }, [login]);

  const toggle = (s: GhSourceInput) =>
    setPicked((prev) => {
      const next = new Map(prev);
      const k = `${s.kind}|${s.key}`;
      if (next.has(k)) next.delete(k);
      else next.set(k, s);
      return next;
    });

  const ql = q.trim().toLowerCase();
  const repoRows = (repos ?? []).filter((r) => !ql || r.fullName.toLowerCase().includes(ql));
  const projRows = (projects ?? []).filter(
    (p) => !ql || p.title.toLowerCase().includes(ql) || p.owner.toLowerCase().includes(ql),
  );
  const nRepos = [...picked.values()].filter((s) => s.kind === "repo").length;
  const nProjects = picked.size - nRepos;

  return (
    <div className="iss-setup iss-setup--fill">
      <div className="iss-setup-title">
        Repos &amp; projects for{" "}
        <button className="iss-account" onClick={onSwitchAccount} title="Switch account">
          @{login}
        </button>
      </div>
      <div className="iss-seg iss-seg--wide">
        <button className={tab === "repos" ? "iss-seg--on" : ""} onClick={() => setTab("repos")}>
          Repos{nRepos ? ` · ${nRepos}` : ""}
        </button>
        <button className={tab === "projects" ? "iss-seg--on" : ""} onClick={() => setTab("projects")}>
          Projects{nProjects ? ` · ${nProjects}` : ""}
        </button>
      </div>
      <input
        className="task-add-input"
        placeholder={tab === "repos" ? "Search repos…" : "Search projects…"}
        value={q}
        onChange={(e) => setQ(e.target.value)}
      />

      <div className="iss-pick-list">
        {tab === "repos" ? (
          repoErr ? (
            <div className="task-error">{repoErr}</div>
          ) : repos === null ? (
            <div className="iss-empty">Loading repos…</div>
          ) : (
            repoRows.map((r) => {
              const s: GhSourceInput = { kind: "repo", key: r.fullName, title: r.fullName, url: r.url };
              return (
                <label key={r.fullName} className="iss-pick">
                  <input type="checkbox" checked={picked.has(`repo|${r.fullName}`)} onChange={() => toggle(s)} />
                  <span className="iss-pick-name">
                    {r.fullName}
                    {r.private && <span className="iss-private">private</span>}
                  </span>
                </label>
              );
            })
          )
        ) : projErr ? (
          <div className="iss-scope-help">
            <div className="task-error">{projErr}</div>
            {account && !account.canProjects && (
              <p className="iss-hint">
                Then reopen this list. Command:
                <code className="iss-cmd">gh auth refresh -s read:project --user {login}</code>
              </p>
            )}
          </div>
        ) : projects === null ? (
          <div className="iss-empty">Loading projects…</div>
        ) : projRows.length === 0 ? (
          <div className="iss-empty">No open projects for this account.</div>
        ) : (
          projRows.map((p) => {
            const s: GhSourceInput = { kind: "project", key: p.id, title: p.title, url: p.url };
            return (
              <label key={p.id} className="iss-pick">
                <input type="checkbox" checked={picked.has(`project|${p.id}`)} onChange={() => toggle(s)} />
                <span className="iss-pick-name">
                  {p.title}
                  <span className="iss-hint">
                    {p.owner} · #{p.number}
                  </span>
                </span>
              </label>
            );
          })
        )}
        {tab === "projects" &&
          notices.map((n, i) => (
            <div key={i} className="iss-hint">
              ⚠ {n}
            </div>
          ))}
      </div>

      <div className="task-detail-foot">
        {onCancel ? (
          <button className="btn btn-sm" onClick={onCancel}>
            Cancel
          </button>
        ) : (
          <span />
        )}
        <button className="btn btn-sm btn-primary" onClick={() => onSave([...picked.values()])}>
          Save {picked.size ? `(${picked.size})` : ""}
        </button>
      </div>
    </div>
  );
}
