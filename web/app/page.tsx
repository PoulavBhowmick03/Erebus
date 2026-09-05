import { Header } from "@/components/Chrome";
import { Hero } from "@/components/Hero";
import { LeakLedger } from "@/components/LeakLedger";
import { Simulation } from "@/components/Simulation";
import { Observer } from "@/components/Observer";
import { Evidence } from "@/components/Evidence";
import { NonClaims } from "@/components/NonClaims";
import { Consume } from "@/components/Consume";
import { Footer } from "@/components/Footer";

export default function Page() {
  return (
    <>
      <Header />
      <main>
        <Hero />
        <LeakLedger />
        <Simulation />
        <Observer />
        <Evidence />
        <NonClaims />
        <Consume />
      </main>
      <Footer />
    </>
  );
}
