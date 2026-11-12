import { type KeyboardEvent, type ReactNode, useState } from "react";
import { Icon, type IconName } from "./Icon";
import { Popover } from "./Popover";

export type Choice<T extends string> = { value: T; text: string; icon?: IconName };

type ChoiceMenuProps<T extends string> = {
  label: string;
  value: T;
  choices: readonly Choice<T>[];
  onChange: (value: T) => void;
  className: string;
  width?: number;
  children: ReactNode;
};

export function ChoiceMenu<T extends string>({ label, value, choices, onChange, className, width = 168, children }: ChoiceMenuProps<T>) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const current = choices.find((choice) => choice.value === value);

  return (
    <>
      <button
        type="button"
        className={className}
        aria-label={`${label}: ${current?.text ?? value}`}
        aria-haspopup="dialog"
        aria-expanded={anchor !== null}
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
      >
        {children}
      </button>
      {anchor && (
        <Popover anchor={anchor} label={label} width={width} onClose={() => setAnchor(null)}>
          <div className="menu" role="radiogroup" aria-label={label}>
            {choices.map((choice) => (
              <button
                key={choice.value}
                type="button"
                role="radio"
                className="menu-item"
                aria-checked={value === choice.value}
                onClick={() => {
                  onChange(choice.value);
                  setAnchor(null);
                  anchor.focus();
                }}
              >
                {choice.icon && <Icon name={choice.icon} />}
                {choice.text}
              </button>
            ))}
          </div>
        </Popover>
      )}
    </>
  );
}

export type MenuAction = { text: string; run: () => void; danger?: boolean };

function moveFocus(event: KeyboardEvent<HTMLDivElement>) {
  const items = [...event.currentTarget.querySelectorAll<HTMLElement>("[role=menuitem]")];
  const at = items.indexOf(document.activeElement as HTMLElement);
  const moves: Record<string, number> = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: items.length - 1 };
  const target = moves[event.key];
  if (target !== undefined && items.length > 0) {
    event.preventDefault();
    items[(target + items.length) % items.length]?.focus();
  }
}

export function OverflowMenu({ label, actions }: { label: string; actions: MenuAction[] }) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  return (
    <>
      <button
        type="button"
        className="btn btn-ghost btn-icon btn-sm overflow-trigger"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={anchor !== null}
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
      >
        <Icon name="more" />
      </button>
      {anchor && (
        <Popover anchor={anchor} label={label} width={208} onClose={() => setAnchor(null)}>
          <div className="menu" role="menu" aria-label={label} onKeyDown={moveFocus}>
            {actions.map((action) => (
              <button
                key={action.text}
                type="button"
                role="menuitem"
                className={action.danger ? "menu-item menu-danger" : "menu-item"}
                onClick={() => {
                  setAnchor(null);
                  action.run();
                }}
              >
                {action.text}
              </button>
            ))}
          </div>
        </Popover>
      )}
    </>
  );
}
