import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // this package is intentionally outside the repo pnpm workspace
  outputFileTracingRoot: import.meta.dirname,
  output: "export",
  images: { unoptimized: true },
  trailingSlash: true,
};

export default nextConfig;
