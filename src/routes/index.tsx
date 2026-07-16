import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/")({
  head: () => ({
    meta: [
      { title: "AIThermal-Rust — AI-Powered Thermal Analysis in Rust" },
      {
        name: "description",
        content:
          "AIThermal-Rust: a blazing-fast, memory-safe thermal analysis engine powered by AI and built in Rust.",
      },
      { property: "og:title", content: "AIThermal-Rust — AI-Powered Thermal Analysis in Rust" },
      {
        property: "og:description",
        content:
          "A blazing-fast, memory-safe thermal analysis engine powered by AI and built in Rust.",
      },
      { property: "og:type", content: "website" },
      { name: "twitter:card", content: "summary_large_image" },
    ],
  }),
  component: Landing,
});

function Landing() {
  return (
    <div className="min-h-screen bg-background text-foreground">
      {/* Nav */}
      <header className="border-b border-border/60">
        <div className="mx-auto flex max-w-6xl items-center justify-between px-6 py-5">
          <div className="flex items-center gap-2 font-mono text-sm font-semibold tracking-tight">
            <span className="inline-flex h-6 w-6 items-center justify-center rounded-md bg-primary text-primary-foreground">
              ⚙
            </span>
            AIThermal<span className="text-muted-foreground">-Rust</span>
          </div>
          <nav className="hidden gap-8 text-sm text-muted-foreground md:flex">
            <a href="#features" className="hover:text-foreground transition-colors">Features</a>
            <a href="#how" className="hover:text-foreground transition-colors">How it works</a>
            <a href="#docs" className="hover:text-foreground transition-colors">Docs</a>
          </nav>
          <a
            href="#get-started"
            className="inline-flex items-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
          >
            Get started
          </a>
        </div>
      </header>

      {/* Hero */}
      <section className="relative overflow-hidden">
        <div
          aria-hidden
          className="pointer-events-none absolute inset-0 opacity-[0.35]"
          style={{
            background:
              "radial-gradient(600px circle at 20% 10%, oklch(0.85 0.12 40 / 0.35), transparent 60%), radial-gradient(500px circle at 80% 30%, oklch(0.75 0.18 20 / 0.28), transparent 60%)",
          }}
        />
        <div className="relative mx-auto max-w-6xl px-6 py-24 md:py-32">
          <div className="max-w-3xl">
            <span className="inline-flex items-center gap-2 rounded-full border border-border bg-card px-3 py-1 text-xs font-medium text-muted-foreground">
              <span className="h-1.5 w-1.5 rounded-full bg-chart-1" />
              v0.1 · early access
            </span>
            <h1 className="mt-6 text-5xl font-semibold tracking-tight md:text-7xl">
              Thermal analysis,{" "}
              <span className="text-muted-foreground">reimagined in Rust.</span>
            </h1>
            <p className="mt-6 max-w-2xl text-lg text-muted-foreground">
              AIThermal-Rust fuses AI-driven inference with a zero-cost,
              memory-safe simulation core — so you can model heat, not fight
              your toolchain.
            </p>
            <div className="mt-10 flex flex-wrap gap-3" id="get-started">
              <a
                href="#"
                className="inline-flex items-center rounded-md bg-primary px-5 py-3 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
              >
                Install with cargo
              </a>
              <a
                href="#docs"
                className="inline-flex items-center rounded-md border border-border bg-card px-5 py-3 text-sm font-medium text-foreground transition-colors hover:bg-accent"
              >
                Read the docs →
              </a>
            </div>
            <pre className="mt-10 w-full max-w-xl overflow-x-auto rounded-lg border border-border bg-card px-4 py-3 font-mono text-sm text-muted-foreground">
              <code>$ cargo add aithermal-rust</code>
            </pre>
          </div>
        </div>
      </section>

      {/* Features */}
      <section id="features" className="border-t border-border/60">
        <div className="mx-auto max-w-6xl px-6 py-24">
          <h2 className="text-3xl font-semibold tracking-tight md:text-4xl">
            Built for engineers who ship.
          </h2>
          <p className="mt-3 max-w-xl text-muted-foreground">
            A focused toolkit for thermal simulation, calibration, and prediction.
          </p>

          <div className="mt-12 grid gap-6 md:grid-cols-3">
            {[
              {
                title: "AI-assisted solvers",
                body: "Neural surrogates accelerate steady-state and transient heat transfer by orders of magnitude.",
              },
              {
                title: "Zero-cost Rust core",
                body: "SIMD-vectorized, no GC, deterministic memory — production-grade performance out of the box.",
              },
              {
                title: "Interoperable",
                body: "Native bindings for Python, C, and WASM. Drop it into existing CFD or MLOps pipelines.",
              },
            ].map((f) => (
              <div
                key={f.title}
                className="rounded-lg border border-border bg-card p-6 transition-colors hover:bg-accent"
              >
                <h3 className="text-lg font-semibold">{f.title}</h3>
                <p className="mt-2 text-sm text-muted-foreground">{f.body}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* How it works */}
      <section id="how" className="border-t border-border/60 bg-secondary/40">
        <div className="mx-auto max-w-6xl px-6 py-24">
          <h2 className="text-3xl font-semibold tracking-tight md:text-4xl">
            Three steps to thermal insight.
          </h2>
          <div className="mt-12 grid gap-6 md:grid-cols-3">
            {[
              ["01", "Define", "Describe geometry, materials, and boundary conditions in TOML or Rust."],
              ["02", "Solve", "Run the hybrid AI + FEM solver on CPU, GPU, or edge devices."],
              ["03", "Predict", "Export fields, generate reports, or stream results to your app."],
            ].map(([n, t, b]) => (
              <div key={n} className="rounded-lg border border-border bg-card p-6">
                <div className="font-mono text-xs text-muted-foreground">{n}</div>
                <h3 className="mt-3 text-lg font-semibold">{t}</h3>
                <p className="mt-2 text-sm text-muted-foreground">{b}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* CTA */}
      <section id="docs" className="border-t border-border/60">
        <div className="mx-auto max-w-4xl px-6 py-24 text-center">
          <h2 className="text-3xl font-semibold tracking-tight md:text-5xl">
            Ready to feel the heat?
          </h2>
          <p className="mx-auto mt-4 max-w-xl text-muted-foreground">
            Join the early access program and help shape the future of open-source thermal simulation.
          </p>
          <div className="mt-8 flex justify-center gap-3">
            <a
              href="#"
              className="inline-flex items-center rounded-md bg-primary px-5 py-3 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
            >
              Request access
            </a>
            <a
              href="#"
              className="inline-flex items-center rounded-md border border-border bg-card px-5 py-3 text-sm font-medium text-foreground transition-colors hover:bg-accent"
            >
              View on GitHub
            </a>
          </div>
        </div>
      </section>

      <footer className="border-t border-border/60">
        <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-2 px-6 py-8 text-sm text-muted-foreground md:flex-row">
          <div>© {new Date().getFullYear()} AIThermal-Rust</div>
          <div className="font-mono text-xs">built with 🦀 + ai</div>
        </div>
      </footer>
    </div>
  );
}
