// Pairview: translation check for src/pv (docs/IMPLEMENTATION_PLAN.md M1-17).
//
//   npm run pv:i18n-check
//
// Fails when Pairview code
//   1. puts hard-coded words on screen: JSX text, a string in a JSX `{…}`, or a string in a
//      user-visible attribute (title, aria-label, placeholder, alt, data-tooltip, label), or
//   2. calls t('pv.…') with a key missing from src/pv/i18n/en.json.
// Format codes and units-free symbols (RAW, HIF, ·, …) pass: only words with a lowercase letter count.
// A line ending in `// pv-i18n-ok` is skipped (for text that is data, not UI).
//
// Written with the TypeScript compiler because upstream's eslint-plugin-i18next rule reports
// nothing under this repo's ESLint version.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const pvDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const en = JSON.parse(fs.readFileSync(path.join(pvDir, 'i18n', 'en.json'), 'utf8'));

const VISIBLE_ATTRIBUTES = new Set(['title', 'aria-label', 'placeholder', 'alt', 'data-tooltip', 'label']);
/** A word a person reads: has a lowercase letter, and isn't a CSS class list or a key. */
const isWords = (text) => /[a-z]/.test(text) && !/^[a-z0-9]+([.:_-][a-zA-Z0-9]+)+$/.test(text.trim());

const hasKey = (key) => {
  let node = en;
  for (const part of key.split('.')) {
    if (node && typeof node === 'object' && part in node) node = node[part];
    else return ['_one', '_other'].some((suffix) => typeof node?.[part + suffix] === 'string');
  }
  return typeof node === 'string' || (node && typeof node === 'object');
};

const files = [];
const walk = (dir) => {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full);
    else if (/\.tsx?$/.test(entry.name)) files.push(full);
  }
};
walk(pvDir);

const problems = [];
for (const file of files) {
  const text = fs.readFileSync(file, 'utf8');
  const lines = text.split('\n');
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true);
  const report = (node, message) => {
    const { line } = source.getLineAndCharacterOfPosition(node.getStart());
    if (lines[line].trimEnd().endsWith('// pv-i18n-ok')) return;
    problems.push(`${path.relative(process.cwd(), file)}:${line + 1}: ${message}`);
  };
  const isStringish = (node) => ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node);

  const visit = (node) => {
    if (ts.isJsxText(node) && isWords(node.text)) report(node, `hard-coded text "${node.text.trim()}"`);

    if (ts.isJsxExpression(node) && node.expression && isStringish(node.expression)) {
      const parent = node.parent;
      const attribute = ts.isJsxAttribute(parent) ? parent.name.getText(source) : null;
      if ((attribute === null || VISIBLE_ATTRIBUTES.has(attribute)) && isWords(node.expression.text))
        report(node, `hard-coded text "${node.expression.text}"`);
    }

    if (ts.isJsxAttribute(node) && node.initializer && ts.isStringLiteral(node.initializer)) {
      const name = node.name.getText(source);
      if (VISIBLE_ATTRIBUTES.has(name) && isWords(node.initializer.text))
        report(node, `hard-coded ${name}="${node.initializer.text}"`);
    }

    if (
      ts.isCallExpression(node) &&
      ts.isIdentifier(node.expression) &&
      node.expression.text === 't' &&
      node.arguments[0] &&
      isStringish(node.arguments[0])
    ) {
      const key = node.arguments[0].text;
      if (key.startsWith('pv.') && !hasKey(key)) report(node, `missing key "${key}" in src/pv/i18n/en.json`);
    }

    ts.forEachChild(node, visit);
  };
  visit(source);
}

if (problems.length > 0) {
  console.error(problems.join('\n'));
  console.error(`\n${problems.length} problem(s). Move the text into src/pv/i18n/en.json and use t('pv.…').`);
  process.exit(1);
}
console.log(`pv i18n check: ${files.length} files clean.`);
