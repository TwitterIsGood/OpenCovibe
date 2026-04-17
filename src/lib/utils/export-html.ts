import { renderMarkdown } from "$lib/utils/markdown";

const EXPORT_CSS = `
body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
  max-width: 800px;
  margin: 0 auto;
  padding: 2rem;
  line-height: 1.7;
  color: #1a1a1a;
  background: #fff;
}
h1 { font-size: 1.5rem; margin-bottom: 0.5rem; }
h2 { font-size: 1.15rem; margin-top: 1.5rem; color: #2563eb; }
h2.user-heading { color: #059669; }
h2.assistant-heading { color: #2563eb; }
h2.tool-heading { color: #9333ea; font-size: 1rem; }
.meta { color: #888; font-size: 0.85rem; margin-bottom: 1rem; }
hr { border: none; border-top: 1px solid #e5e7eb; margin: 1.5rem 0; }
pre {
  background: #0d1117;
  color: #c9d1d9;
  padding: 1rem;
  border-radius: 6px;
  overflow-x: auto;
  font-size: 0.875rem;
  line-height: 1.5;
}
code {
  font-family: 'SF Mono', Consolas, 'Liberation Mono', Menlo, monospace;
  font-size: 0.875rem;
}
:not(pre) > code {
  background: #f1f5f9;
  padding: 0.15em 0.35em;
  border-radius: 4px;
  color: #be185d;
}
a { color: #2563eb; }
table { border-collapse: collapse; width: 100%; margin: 1rem 0; }
th, td { border: 1px solid #d1d5db; padding: 0.5rem 0.75rem; text-align: left; }
th { background: #f3f4f6; font-weight: 600; }
blockquote { border-left: 3px solid #d1d5db; padding-left: 1rem; color: #6b7280; margin: 1rem 0; }
img { max-width: 100%; height: auto; }

/* highlight.js github-dark theme (inline) */
.hljs { color: #c9d1d9; background: #0d1117; }
.hljs-comment, .hljs-quote { color: #8b949e; font-style: italic; }
.hljs-keyword, .hljs-selector-tag, .hljs-type { color: #ff7b72; }
.hljs-string, .hljs-addition { color: #a5d6ff; }
.hljs-number, .hljs-literal { color: #79c0ff; }
.hljs-built_in { color: #ffa657; }
.hljs-function .hljs-title { color: #d2a8ff; }
.hljs-title { color: #d2a8ff; }
.hljs-attr, .hljs-variable { color: #79c0ff; }
.hljs-symbol, .hljs-bullet { color: #f2cc60; }
.hljs-meta { color: #79c0ff; }
.hljs-deletion { color: #ffa198; background: rgba(248,81,73,0.1); }
.hljs-name { color: #7ee787; }
.hljs-section { color: #d2a8ff; font-weight: bold; }
.hljs-selector-class { color: #7ee787; }
.hljs-selector-id { color: #7ee787; }
`;

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function buildExportHtml(
  title: string,
  markdownBody: string,
  generatedAt: string,
): string {
  // Add role-specific CSS classes to headings
  const processedMd = markdownBody
    .replace(
      /^## (User)$/gm,
      '<h2 class="user-heading">User</h2>',
    )
    .replace(
      /^## (Assistant)$/gm,
      '<h2 class="assistant-heading">Assistant</h2>',
    )
    .replace(
      /^## (Tool:.*)$/gm,
      '<h2 class="tool-heading">$1</h2>',
    );

  const bodyHtml = renderMarkdown(processedMd);

  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>${escapeHtml(title)}</title>
<style>${EXPORT_CSS}</style>
</head>
<body>
<h1>${escapeHtml(title)}</h1>
<p class="meta">Generated: ${escapeHtml(generatedAt)}</p>
<hr>
${bodyHtml}
</body>
</html>`;
}
