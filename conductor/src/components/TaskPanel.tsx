// Tasks sidebar: a lightweight dev to-do list with a product workflow
// (Backlog → To do → In progress → Review → Done, plus Blocked). Enter adds a task to
// To do, ⌘Enter starts it straight away. Each task keeps a timeline of status changes
// (owned by the backend) so lead/cycle time can be shown without extra bookkeeping.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createTask, deleteTask, listTasks, taskEvents, updateTask } from "../ipc/api";
import type { Task, TaskEvent, TaskPriority, TaskStatus } from "../ipc/types";
import { getProjectColor, useProjectColors } from "../lib/projectColors";

interface TaskPanelProps {
  /** Active terminal's project — new tasks are tagged with it. */
  project: string | null;
  onClose: () => void;
}

const DAY_MS = 86_400_000;
/** How far back finished tasks stay visible in the Done section. */
const DONE_WINDOW_DAYS = 7;
const TICK_MS = 60_000;
const COLLAPSED_KEY = "conductor-tasks-collapsed";
const FILTER_KEY = "conductor-tasks-this-project";

const STATUS_LABEL: Record<TaskStatus, string> = {
  in_progress: "In progress",
  blocked: "Blocked",
  review: "Review",
  todo: "To do",
  backlog: "Backlog",
  done: "Done",
};
/** Section order in the panel. */
const SECTIONS: TaskStatus[] = ["in_progress", "blocked", "review", "todo", "backlog", "done"];
/** What a click on the status dot moves a task to. */
const NEXT_STATUS: Record<TaskStatus, TaskStatus> = {
  backlog: "todo",
  todo: "in_progress",
  in_progress: "review",
  review: "done",
  blocked: "in_progress",
  done: "todo",
};
const PRIORITY_LABEL: Record<TaskPriority, string> = { 0: "Urgent", 1: "High", 2: "Normal", 3: "Low" };

function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** "2026-10-08" (local) → end of that local day, epoch ms. */
function dateInputToDue(v: string): number | null {
  if (!v) return null;
  const [y, m, d] = v.split("-").map(Number);
  return new Date(y, m - 1, d, 23, 59, 59, 999).getTime();
}

function dueToDateInput(ms: number | null): string {
  if (ms == null) return "";
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function fmtDuration(ms: number): string {
  const m = Math.max(0, Math.floor(ms / 60_000));
  if (m < 1) return "<1m";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d}d ${h % 24}h` : `${d}d`;
}

function fmtDay(ms: number, now: number): string {
  const diff = Math.round((startOfDay(ms) - startOfDay(now)) / DAY_MS);
  if (diff === 0) return "Today";
  if (diff === -1) return "Yesterday";
  if (diff === 1) return "Tomorrow";
  if (diff > 1 && diff < 7) return new Date(ms).toLocaleDateString(undefined, { weekday: "short" });
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function fmtTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

function fmtStamp(ms: number, now: number): string {
  return `${fmtDay(ms, now)} ${fmtTime(ms)}`;
}

/** Due-chip text + tone: overdue / today / later. */
function dueInfo(t: Task, now: number): { text: string; tone: "overdue" | "today" | "later" } | null {
  if (t.dueAt == null || t.status === "done") return null;
  const today = startOfDay(now);
  if (t.dueAt < today) return { text: `Overdue · ${fmtDay(t.dueAt, now)}`, tone: "overdue" };
  if (t.dueAt < today + DAY_MS) return { text: "Due today", tone: "today" };
  return { text: `Due ${fmtDay(t.dueAt, now)}`, tone: "later" };
}

/** Age shown on the row: how long it's been in its current state. */
function ageText(t: Task, now: number): string | null {
  if (t.status === "done" && t.completedAt) return `✓ ${fmtStamp(t.completedAt, now)}`;
  if (t.status === "in_progress" && t.startedAt) return `${fmtDuration(now - t.startedAt)} in progress`;
  if (t.status === "todo" || t.status === "backlog") return `added ${fmtDay(t.createdAt, now).toLowerCase()}`;
  return null;
}

function sortTasks(status: TaskStatus, list: Task[]): Task[] {
  if (status === "done") return [...list].sort((a, b) => (b.completedAt ?? 0) - (a.completedAt ?? 0));
  return [...list].sort(
    (a, b) =>
      a.priority - b.priority ||
      (a.dueAt ?? Infinity) - (b.dueAt ?? Infinity) ||
      b.createdAt - a.createdAt,
  );
}

function loadCollapsed(): Set<TaskStatus> {
  try {
    const v = localStorage.getItem(COLLAPSED_KEY);
    if (v) return new Set(JSON.parse(v) as TaskStatus[]);
  } catch {
    /* ignore */
  }
  return new Set<TaskStatus>(["backlog"]);
}

export function TaskPanel({ project, onClose }: TaskPanelProps) {
  useProjectColors(); // re-render on color changes
  const [tasks, setTasks] = useState<Task[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<TaskStatus>>(loadCollapsed);
  const [thisProject, setThisProject] = useState(() => {
    try {
      return localStorage.getItem(FILTER_KEY) === "1";
    } catch {
      return false;
    }
  });
  const [now, setNow] = useState(() => Date.now());
  const inputRef = useRef<HTMLInputElement>(null);

  // Relative times ("2h in progress") stay fresh without reloading.
  useEffect(() => {
    const iv = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(iv);
  }, []);

  useEffect(() => {
    let active = true;
    listTasks(startOfDay(Date.now()) - (DONE_WINDOW_DAYS - 1) * DAY_MS)
      .then((list) => active && setTasks(list))
      .catch((e) => active && setError(String(e)));
    inputRef.current?.focus();
    return () => {
      active = false;
    };
  }, []);

  const upsert = useCallback((t: Task) => {
    setTasks((prev) => {
      const i = prev.findIndex((x) => x.id === t.id);
      if (i < 0) return [t, ...prev];
      const next = prev.slice();
      next[i] = t;
      return next;
    });
  }, []);

  const save = useCallback(
    (t: Task) => {
      updateTask(t)
        .then((saved) => {
          upsert(saved);
          setError(null);
        })
        .catch((e) => setError(String(e)));
    },
    [upsert],
  );

  const add = useCallback(
    (status: TaskStatus) => {
      const title = draft.trim();
      if (!title) return;
      const t: Task = {
        id: "",
        title,
        notes: "",
        status,
        priority: 2,
        project,
        dueAt: null,
        createdAt: 0,
        updatedAt: 0,
        startedAt: null,
        completedAt: null,
      };
      createTask(t)
        .then((saved) => {
          upsert(saved);
          setDraft("");
          setError(null);
        })
        .catch((e) => setError(String(e)));
    },
    [draft, project, upsert],
  );

  const remove = useCallback((id: string) => {
    deleteTask(id)
      .then(() => {
        setTasks((prev) => prev.filter((t) => t.id !== id));
        setExpanded((e) => (e === id ? null : e));
      })
      .catch((e) => setError(String(e)));
  }, []);

  const toggleSection = (s: TaskStatus) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(s)) next.delete(s);
      else next.add(s);
      try {
        localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]));
      } catch {
        /* ignore */
      }
      return next;
    });

  const toggleFilter = () =>
    setThisProject((v) => {
      try {
        localStorage.setItem(FILTER_KEY, v ? "0" : "1");
      } catch {
        /* ignore */
      }
      return !v;
    });

  const filterOn = thisProject && !!project;
  const visible = useMemo(
    () => (filterOn ? tasks.filter((t) => t.project === project) : tasks),
    [tasks, filterOn, project],
  );
  const bySection = useMemo(() => {
    const m = new Map<TaskStatus, Task[]>();
    for (const s of SECTIONS) m.set(s, sortTasks(s, visible.filter((t) => t.status === s)));
    return m;
  }, [visible]);

  const today = startOfDay(now);
  const doneToday = visible.filter((t) => t.status === "done" && (t.completedAt ?? 0) >= today).length;
  const inProgress = bySection.get("in_progress")?.length ?? 0;
  const overdue = visible.filter((t) => t.status !== "done" && t.dueAt != null && t.dueAt < today).length;

  return (
    <div className="task-panel">
      <div className="task-head">
        <div className="task-title">
          <span className="task-stats">
            {inProgress} active · {doneToday} done today
            {overdue > 0 && <span className="task-overdue-count"> · {overdue} overdue</span>}
          </span>
        </div>
        <button className="git-x" onClick={onClose} title="Close tasks">
          ✕
        </button>
      </div>

      <div className="task-add">
        <input
          ref={inputRef}
          className="task-add-input"
          placeholder="Add a task…"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add(e.metaKey ? "in_progress" : "todo");
            } else if (e.key === "Escape") {
              setDraft("");
              e.currentTarget.blur();
            }
          }}
        />
        <div className="task-add-hint">
          <span>
            ↵ to do · ⌘↵ start now
            {project && (
              <>
                {" · "}
                <span className="task-proj-dot" style={{ background: getProjectColor(project) }} />
                {project}
              </>
            )}
          </span>
          {project && (
            <button
              className={`task-filter ${filterOn ? "task-filter--on" : ""}`}
              onClick={toggleFilter}
              title="Show only this project's tasks"
            >
              {filterOn ? "This project" : "All projects"}
            </button>
          )}
        </div>
        {error && <div className="task-error">{error}</div>}
      </div>

      <div className="task-list">
        {visible.length === 0 && (
          <div className="task-empty">Nothing here yet. Type above and press Enter.</div>
        )}
        {SECTIONS.map((s) => {
          const list = bySection.get(s) ?? [];
          if (list.length === 0) return null;
          const isCollapsed = collapsed.has(s);
          return (
            <section key={s} className="task-section">
              <button className="task-section-h" onClick={() => toggleSection(s)}>
                <span className={`task-caret ${isCollapsed ? "" : "task-caret--open"}`}>▸</span>
                <span className={`task-dot task-dot--${s}`} />
                {s === "done" ? `Done · last ${DONE_WINDOW_DAYS} days` : STATUS_LABEL[s]}
                <span className="task-count">{list.length}</span>
              </button>
              {!isCollapsed &&
                list.map((t) => (
                  <TaskRow
                    key={t.id}
                    task={t}
                    now={now}
                    showProject={!filterOn}
                    open={expanded === t.id}
                    onToggleOpen={() => setExpanded((e) => (e === t.id ? null : t.id))}
                    onSave={save}
                    onDelete={remove}
                  />
                ))}
            </section>
          );
        })}
      </div>
    </div>
  );
}

interface TaskRowProps {
  task: Task;
  now: number;
  showProject: boolean;
  open: boolean;
  onToggleOpen: () => void;
  onSave: (t: Task) => void;
  onDelete: (id: string) => void;
}

function TaskRow({ task: t, now, showProject, open, onToggleOpen, onSave, onDelete }: TaskRowProps) {
  const due = dueInfo(t, now);
  const age = ageText(t, now);
  return (
    <div className={`task-row ${open ? "task-row--open" : ""} task-row--${t.status}`}>
      <div className="task-row-main">
        <button
          className={`task-status task-status--${t.status}`}
          onClick={() => onSave({ ...t, status: NEXT_STATUS[t.status] })}
          title={`${STATUS_LABEL[t.status]} → ${STATUS_LABEL[NEXT_STATUS[t.status]]}`}
        >
          {t.status === "done" ? "✓" : t.status === "blocked" ? "!" : ""}
        </button>
        <button className="task-row-body" onClick={onToggleOpen}>
          <span className="task-row-title">
            {t.priority < 2 && (
              <span className={`task-prio task-prio--${t.priority}`}>P{t.priority}</span>
            )}
            {t.title}
          </span>
          {(age || due || (showProject && t.project)) && (
            <span className="task-meta">
              {due && <span className={`task-due task-due--${due.tone}`}>{due.text}</span>}
              {age && <span>{age}</span>}
              {showProject && t.project && (
                <span className="task-proj">
                  <span className="task-proj-dot" style={{ background: getProjectColor(t.project) }} />
                  {t.project}
                </span>
              )}
            </span>
          )}
        </button>
      </div>
      {open && <TaskDetail task={t} now={now} onSave={onSave} onDelete={onDelete} />}
    </div>
  );
}

interface TaskDetailProps {
  task: Task;
  now: number;
  onSave: (t: Task) => void;
  onDelete: (id: string) => void;
}

function TaskDetail({ task: t, now, onSave, onDelete }: TaskDetailProps) {
  const [title, setTitle] = useState(t.title);
  const [notes, setNotes] = useState(t.notes);
  const [events, setEvents] = useState<TaskEvent[]>([]);
  const [armDelete, setArmDelete] = useState(false);

  useEffect(() => setTitle(t.title), [t.title]);
  useEffect(() => setNotes(t.notes), [t.notes]);
  // Reload the timeline whenever the status (and so the history) changes.
  useEffect(() => {
    let active = true;
    taskEvents(t.id)
      .then((ev) => active && setEvents(ev))
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [t.id, t.status]);

  const commitTitle = () => {
    const v = title.trim();
    if (v && v !== t.title) onSave({ ...t, title: v });
    else setTitle(t.title);
  };

  const lead = t.completedAt ? t.completedAt - t.createdAt : null;
  const cycle = t.completedAt && t.startedAt ? t.completedAt - t.startedAt : null;

  return (
    <div className="task-detail">
      <input
        className="task-field task-field--title"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
        onBlur={commitTitle}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
      />
      <div className="task-detail-grid">
        <label>
          Status
          <select
            className="task-field"
            value={t.status}
            onChange={(e) => onSave({ ...t, status: e.target.value as TaskStatus })}
          >
            {(["backlog", "todo", "in_progress", "review", "blocked", "done"] as TaskStatus[]).map((s) => (
              <option key={s} value={s}>
                {STATUS_LABEL[s]}
              </option>
            ))}
          </select>
        </label>
        <label>
          Priority
          <select
            className="task-field"
            value={t.priority}
            onChange={(e) => onSave({ ...t, priority: Number(e.target.value) as TaskPriority })}
          >
            {([0, 1, 2, 3] as TaskPriority[]).map((p) => (
              <option key={p} value={p}>
                P{p} · {PRIORITY_LABEL[p]}
              </option>
            ))}
          </select>
        </label>
        <label>
          Due
          <input
            type="date"
            className="task-field"
            value={dueToDateInput(t.dueAt)}
            onChange={(e) => onSave({ ...t, dueAt: dateInputToDue(e.target.value) })}
          />
        </label>
      </div>
      <textarea
        className="task-field task-notes"
        placeholder="Notes, links, acceptance criteria…"
        value={notes}
        onChange={(e) => setNotes(e.target.value)}
        onBlur={() => notes !== t.notes && onSave({ ...t, notes })}
      />

      <div className="task-timeline">
        {events.map((ev, i) => (
          <div key={i} className="task-tl-item">
            <span className={`task-dot task-dot--${ev.toStatus}`} />
            <span className="task-tl-what">
              {ev.fromStatus == null ? `Created in ${STATUS_LABEL[ev.toStatus]}` : STATUS_LABEL[ev.toStatus]}
            </span>
            <span className="task-tl-when">{fmtStamp(ev.at, now)}</span>
          </div>
        ))}
      </div>

      <div className="task-detail-foot">
        <span className="task-metrics">
          {lead != null && <>Lead {fmtDuration(lead)}</>}
          {cycle != null && <> · Cycle {fmtDuration(cycle)}</>}
          {lead == null && <>Open {fmtDuration(now - t.createdAt)}</>}
        </span>
        {/* Two-step delete: first click arms, second click deletes. */}
        <button
          className={`task-delete ${armDelete ? "task-delete--armed" : ""}`}
          onClick={() => (armDelete ? onDelete(t.id) : setArmDelete(true))}
          onBlur={() => setArmDelete(false)}
        >
          {armDelete ? "Click to confirm" : "Delete"}
        </button>
      </div>
    </div>
  );
}
