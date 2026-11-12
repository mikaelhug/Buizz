export type Crumb = { text: string; href?: string };

export function Breadcrumbs({ trail }: { trail: Crumb[] }) {
  return (
    <nav className="crumbs" aria-label="Breadcrumb">
      <ol>
        {trail.map((crumb, index) => (
          <li key={crumb.text}>{crumb.href && index < trail.length - 1 ? <a href={crumb.href}>{crumb.text}</a> : <span aria-current="page">{crumb.text}</span>}</li>
        ))}
      </ol>
    </nav>
  );
}
