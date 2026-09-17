import type { Metadata } from "next";
import { Header, Section } from "@/components/Chrome";
import { Footer } from "@/components/Footer";
import { DOCS_PAGES } from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus · Not found",
};

export default function NotFound() {
  return (
    <>
      <Header />
      <main>
        <Section className="pt-28 pb-24 md:pt-36">
          <div className="max-w-[60ch]">
            <p className="label m-0 mb-6">404</p>
            <h1 className="display mb-0 text-[clamp(36px,6vw,80px)]">Not found.</h1>
            <p className="lead mt-8 max-w-[52ch]">
              Nothing lives at this address. The page moved, or the link was wrong to begin with.
            </p>

            <div className="mt-12 border-t border-rule pt-8">
              <p className="label m-0 mb-4">Try instead</p>
              <ul className="m-0 list-none space-y-3 p-0">
                <li>
                  <a href="/" className="link">
                    Erebus ↗
                  </a>
                </li>
                {DOCS_PAGES.map((p) => (
                  <li key={p.href}>
                    <a href={p.href} className="link">
                      {p.label} ↗
                    </a>
                  </li>
                ))}
              </ul>
            </div>
          </div>
        </Section>
      </main>
      <Footer />
    </>
  );
}
