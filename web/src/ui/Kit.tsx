import type { ReactNode } from "react";

type Tone = "up" | "down" | undefined;

export function Card({ title, aside, label, className, children }: { title?: ReactNode; aside?: ReactNode; label?: string; className?: string; children: ReactNode }) {
  return (
    <section className={className ? `card ${className}` : "card"} aria-label={label}>
      {(title || aside) && (
        <header className="card-head">
          {title && <h2 className="card-title">{title}</h2>}
          {aside && <div className="card-aside">{aside}</div>}
        </header>
      )}
      {children}
    </section>
  );
}

export function Stat({ label, value, tone, sub, size = "md" }: { label: ReactNode; value: string; tone?: Tone; sub?: ReactNode; size?: "md" | "lg" }) {
  return (
    <div className={`stat stat-${size}`}>
      <div className="stat-label">{label}</div>
      <div className={`stat-value num ${tone ?? ""}`}>{value}</div>
      {sub && <div className="stat-sub">{sub}</div>}
    </div>
  );
}

export function EmptyState({ title, children, action }: { title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty-state">
      <p className="empty-state-title">{title}</p>
      {children && <p className="empty-state-body">{children}</p>}
      {action}
    </div>
  );
}
