import { Header } from "@/components/Chrome";
import { Hero } from "@/components/Hero";
import { Proof } from "@/components/Proof";
import { PoolBand } from "@/components/PoolBand";
import { Replay } from "@/components/Replay";
import { Boundary } from "@/components/Boundary";
import { Footer } from "@/components/Footer";

export default function Page() {
  return (
    <>
      <Header />
      <main>
        <Hero />
        <Proof />
        <PoolBand />
        <Replay />
        <Boundary />
      </main>
      <Footer />
    </>
  );
}
