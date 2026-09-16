export function DocSection({
  id,
  n,
  title,
  children,
}: {
  id: string;
  n: string;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section
      id={id}
      className="doc-step grid scroll-mt-24 grid-cols-1 gap-6 border-t border-rule py-12 md:grid-cols-[4rem_1fr] md:gap-10"
    >
      <span className="mono-xs pt-2 uppercase tracking-[0.16em] text-fore-3">{n}</span>
      <div>
        <h2 className="display m-0 mb-6 text-[clamp(21px,2.4vw,34px)] leading-[1.05]">{title}</h2>
        {children}
      </div>
    </section>
  );
}
