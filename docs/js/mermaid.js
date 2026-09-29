// Renders ```mermaid blocks as diagrams with Mermaid, loaded from a CDN only
// on pages that have one. Without it (offline, blocked CDN) the diagram
// source stays visible as a code block; the .md pages always carry the source.
(async () => {
  const blocks = document.querySelectorAll("code.language-mermaid");
  if (blocks.length === 0) return;
  const { default: mermaid } = await import("https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs");
  const dark = ["coal", "navy", "ayu"].some((theme) => document.documentElement.classList.contains(theme));
  mermaid.initialize({ startOnLoad: false, theme: dark ? "dark" : "default" });
  for (const code of blocks) {
    const diagram = document.createElement("pre");
    diagram.className = "mermaid";
    diagram.textContent = code.textContent;
    code.parentElement.replaceWith(diagram);
  }
  await mermaid.run();
})();
