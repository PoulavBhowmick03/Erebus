import { Header, Section } from "@/components/Chrome";
import { Footer } from "@/components/Footer";
import { DocsToc } from "@/components/DocsToc";
import { DocsPager } from "@/components/DocsPager";

export default function DocsLayout({ children }: { children: React.ReactNode }) {
  return (
    <>
      <Header />
      <main className="plate relative">
        <div
          aria-hidden
          className="pointer-events-none absolute inset-x-0 top-0 h-[720px]"
          style={{
            background:
              "radial-gradient(55% 55% at 100% 0%, rgba(251, 64, 32, 0.07), transparent 70%), " +
              "radial-gradient(45% 40% at 46% 0%, rgba(251, 64, 32, 0.045), transparent 72%)",
          }}
        />
        <Section className="relative pt-28 pb-8 md:pt-36">
          <div className="mx-auto grid max-w-[1240px] grid-cols-1 gap-x-16 lg:grid-cols-[12rem_1fr]">
            <DocsToc />
            <div className="min-w-0 max-w-[1080px]">
              {children}
              <DocsPager />
            </div>
          </div>
        </Section>
      </main>
      <Footer />
    </>
  );
}
