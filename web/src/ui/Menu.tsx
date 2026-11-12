import { type ReactNode, useState } from "react";
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
