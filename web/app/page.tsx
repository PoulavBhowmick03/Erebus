import { Header } from "@/components/Chrome";
import { Hero } from "@/components/Hero";
import { Proof } from "@/components/Proof";
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
        <Replay />
        <Boundary />
      </main>
      <Footer />
    </>
  );
}
