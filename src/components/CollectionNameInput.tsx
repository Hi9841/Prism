import { useId, useState } from "react";

export function CollectionNameInput({ name, onCommit }: { name: string; onCommit: (name: string) => void }) {
  const [draft, setDraft] = useState(name);
  const [invalid, setInvalid] = useState(false);
  const errorId = useId();
  const commit = () => {
    const trimmed = draft.trim();
    setInvalid(!trimmed);
    if (!trimmed) return;
    setDraft(trimmed);
    if (trimmed !== name) onCommit(trimmed);
  };

  return (
    <div className="min-w-0 flex-1">
      <input
        value={draft}
        maxLength={64}
        aria-label={`${name} collection name`}
        aria-invalid={invalid}
        aria-describedby={invalid ? errorId : undefined}
        onChange={(event) => {
          setDraft(event.target.value);
          setInvalid(false);
        }}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
          } else if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            setDraft(name);
            setInvalid(false);
          }
        }}
        className="focus-ring min-h-8 w-full min-w-0 bg-transparent text-[12px] font-semibold text-fg outline-none"
      />
      {invalid ? (
        <p id={errorId} className="mt-1 text-[11.5px] text-danger">
          Enter a collection name.
        </p>
      ) : null}
    </div>
  );
}
