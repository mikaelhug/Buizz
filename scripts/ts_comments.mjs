import { readFileSync } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(new URL("../web/package.json", import.meta.url));
const ts = require("typescript");

let found = false;
for (const path of process.argv.slice(2)) {
  const text = readFileSync(path, "utf8");
  const source = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true);
  const ranges = new Map();
  const jsxText = [];
  const collect = (list) => {
    for (const range of list ?? []) ranges.set(range.pos, range);
  };
  const visit = (node) => {
    if (ts.isJsxText(node)) {
      jsxText.push([node.getFullStart(), node.getEnd()]);
      return;
    }
    collect(ts.getLeadingCommentRanges(text, node.getFullStart()));
    collect(ts.getTrailingCommentRanges(text, node.getEnd()));
    node.getChildren(source).forEach(visit);
  };
  visit(source);
  for (const range of [...ranges.values()].sort((a, b) => a.pos - b.pos)) {
    if (jsxText.some(([start, end]) => range.pos >= start && range.pos < end)) continue;
    const line = source.getLineAndCharacterOfPosition(range.pos).line + 1;
    const comment = text.slice(range.pos, range.end).split("\n")[0];
    if (line === 1 && comment.startsWith("#!")) continue;
    console.log(`${path}:${line}:${comment}`);
    found = true;
  }
}
process.exit(found ? 1 : 0);
