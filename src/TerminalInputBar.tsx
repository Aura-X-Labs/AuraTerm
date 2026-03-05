import { useState, useRef, useCallback, type KeyboardEvent, type MouseEvent } from "react";
import type { QuickButton } from "./settings";

interface TerminalInputBarProps {
  quickButtons: QuickButton[];
  onSend: (text: string) => void;
  onButtonsChange: (buttons: QuickButton[]) => void;
}

/** textarea 区域高度 < 这个值时自动折叠到 0 */
const SNAP_COLLAPSE_PX = 28;
/** 默认 textarea 高度（对应约 5 行） */
const DEFAULT_TEXTAREA_H = 90;

export function TerminalInputBar({ quickButtons, onSend, onButtonsChange }: TerminalInputBarProps) {
  const [text, setText] = useState("");
  const [showEditor, setShowEditor] = useState(false);
  const [editButtons, setEditButtons] = useState<QuickButton[]>([]);
  const [textareaH, setTextareaH] = useState(DEFAULT_TEXTAREA_H);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const dragStartY = useRef(0);
  const dragStartH = useRef(0);

  // ── Resize handle drag ──────────────────────────────────────────────────────
  const handleResizeMouseDown = useCallback((e: MouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    dragStartY.current = e.clientY;
    dragStartH.current = textareaH;

    const onMove = (ev: globalThis.MouseEvent) => {
      // 向上拖 → 增大高度；向下拖 → 减小高度
      const delta = dragStartY.current - ev.clientY;
      const newH = Math.max(0, dragStartH.current + delta);
      setTextareaH(newH < SNAP_COLLAPSE_PX ? 0 : newH);
    };

    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    };

    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }, [textareaH]);

  // 双击 handle：折叠 ↔ 展开
  const handleResizeDblClick = () => {
    setTextareaH((h) => (h === 0 ? DEFAULT_TEXTAREA_H : 0));
  };

  const doSend = (payload: string) => {
    if (!payload.trim()) return;
    onSend(payload.endsWith("\n") ? payload : payload + "\n");
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      doSend(text);
      setText("");
    }
  };

  const handleQuickButton = (btn: QuickButton) => {
    doSend(btn.command);
    textareaRef.current?.focus();
  };

  // ── Editor helpers ──────────────────────────────────────────────────────────
  const openEditor = () => {
    setEditButtons(structuredClone(quickButtons));
    setShowEditor(true);
  };

  const saveEditor = () => {
    onButtonsChange(editButtons.filter((b) => b.label.trim() || b.command.trim()));
    setShowEditor(false);
  };

  const addButton = () => {
    setEditButtons((prev) => [
      ...prev,
      { id: crypto.randomUUID(), label: "", command: "" },
    ]);
  };

  const updateButton = (id: string, field: "label" | "command", value: string) => {
    setEditButtons((prev) =>
      prev.map((b) => (b.id === id ? { ...b, [field]: value } : b))
    );
  };

  const deleteButton = (id: string) => {
    setEditButtons((prev) => prev.filter((b) => b.id !== id));
  };

  const moveButton = (id: string, dir: -1 | 1) => {
    setEditButtons((prev) => {
      const idx = prev.findIndex((b) => b.id === id);
      if (idx < 0) return prev;
      const next = idx + dir;
      if (next < 0 || next >= prev.length) return prev;
      const arr = [...prev];
      [arr[idx], arr[next]] = [arr[next], arr[idx]];
      return arr;
    });
  };

  return (
    <div className="terminal-input-bar">
      {/* ── Resize handle ─────────────────────────────────────────────────── */}
      <div
        className={`terminal-input-resize-handle${textareaH === 0 ? " collapsed" : ""}`}
        onMouseDown={handleResizeMouseDown}
        onDoubleClick={handleResizeDblClick}
        title="拖拽调整高度，双击折叠/展开"
      >
        <span className="terminal-input-resize-grip" />
      </div>

      {/* ── Quick buttons row ─────────────────────────────────────────────── */}
      <div className="quick-buttons-bar">
        {quickButtons.map((btn) => (
          <button
            key={btn.id}
            className="quick-btn"
            onClick={() => handleQuickButton(btn)}
            title={btn.command}
            type="button"
          >
            {btn.label.trim() || btn.command.slice(0, 20)}
          </button>
        ))}
        <button
          className="quick-btn quick-btn--edit"
          onClick={openEditor}
          title="Edit quick buttons"
          type="button"
        >
          ✎ Edit
        </button>
      </div>

      {/* ── Textarea + send button ─────────────────────────────────────────── */}
      {textareaH > 0 && (
        <div className="terminal-input-row">
          <textarea
            ref={textareaRef}
            className="terminal-input-textarea"
            style={{ height: textareaH }}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Type here…  Ctrl+Enter (⌘+Enter on macOS) to send"
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
          />
          <button
            className="terminal-input-send-btn"
            onClick={() => { doSend(text); setText(""); }}
            title="Send  (Ctrl+Enter)"
            type="button"
          >
            ▶
          </button>
        </div>
      )}

      {/* ── Quick-button editor dialog ─────────────────────────────────────── */}
      {showEditor && (
        <div
          className="quick-btn-editor-overlay"
          onClick={() => setShowEditor(false)}
        >
          <div
            className="quick-btn-editor"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="quick-btn-editor-header">
              <span>Edit Quick Buttons</span>
              <button
                type="button"
                className="quick-btn-editor-close"
                onClick={() => setShowEditor(false)}
              >
                ×
              </button>
            </div>

            <div className="quick-btn-editor-body">
              {editButtons.length === 0 && (
                <p className="quick-btn-editor-empty">
                  No buttons yet — click <strong>+ Add</strong> to create one.
                </p>
              )}
              {editButtons.map((btn, idx) => (
                <div key={btn.id} className="quick-btn-editor-row">
                  <div className="quick-btn-editor-order">
                    <button
                      type="button"
                      disabled={idx === 0}
                      onClick={() => moveButton(btn.id, -1)}
                      title="Move up"
                    >
                      ▲
                    </button>
                    <button
                      type="button"
                      disabled={idx === editButtons.length - 1}
                      onClick={() => moveButton(btn.id, 1)}
                      title="Move down"
                    >
                      ▼
                    </button>
                  </div>
                  <input
                    type="text"
                    placeholder="Label"
                    value={btn.label}
                    onChange={(e) => updateButton(btn.id, "label", e.target.value)}
                    className="quick-btn-editor-input quick-btn-editor-input--label"
                  />
                  <input
                    type="text"
                    placeholder="Command  (e.g.  ls -la)"
                    value={btn.command}
                    onChange={(e) => updateButton(btn.id, "command", e.target.value)}
                    className="quick-btn-editor-input quick-btn-editor-input--command"
                  />
                  <button
                    type="button"
                    className="quick-btn-editor-delete"
                    onClick={() => deleteButton(btn.id)}
                    title="Delete"
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>

            <div className="quick-btn-editor-footer">
              <button
                type="button"
                className="quick-btn-editor-add"
                onClick={addButton}
              >
                + Add
              </button>
              <span style={{ flex: 1 }} />
              <button
                type="button"
                className="quick-btn-editor-cancel"
                onClick={() => setShowEditor(false)}
              >
                Cancel
              </button>
              <button
                type="button"
                className="quick-btn-editor-save"
                onClick={saveEditor}
              >
                Save
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
