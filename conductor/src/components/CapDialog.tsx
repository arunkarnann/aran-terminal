import type { SessionId, SessionMeta } from "../ipc/types";
import type { CapDialogState } from "../stores/useSessionManager";

interface CapDialogProps {
  capDialog: CapDialogState;
  sessions: SessionMeta[];
  onRaise: () => void;
  onCloseSession: (id: SessionId) => void;
  onDismiss: () => void;
}

export function CapDialog({
  capDialog,
  sessions,
  onRaise,
  onCloseSession,
  onDismiss,
}: CapDialogProps) {
  return (
    <div className="dialog-overlay" onClick={onDismiss}>
      <div className="dialog" onClick={(e) => e.stopPropagation()}>
        <h2 className="dialog-title">Focus limit reached</h2>
        <p className="dialog-body">
          You're at your focus limit ({capDialog.current}/{capDialog.limit}).
          Close a session or raise the limit to open another.
        </p>

        {sessions.length > 0 && (
          <div className="dialog-sessions">
            <p className="dialog-label">Active sessions:</p>
            {sessions.map((s) => (
              <div key={s.id} className="dialog-session-row">
                <span className="dialog-session-name">
                  {s.name ?? `Session ${s.id.slice(0, 8)}`}
                </span>
                <button
                  className="dialog-close-btn"
                  onClick={() => onCloseSession(s.id)}
                >
                  Close
                </button>
              </div>
            ))}
          </div>
        )}

        <div className="dialog-actions">
          <button className="btn btn-primary" onClick={onRaise}>
            Raise limit to {capDialog.limit + 1}
          </button>
          <button className="btn" onClick={onDismiss}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
