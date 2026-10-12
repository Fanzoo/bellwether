import { execFileSync } from 'node:child_process';
import fs from 'node:fs';

/** Label and ProgramArguments of a plist (XML, or binary through `plutil`, read-only). */
export function readPlist(file) {
  let text;
  try {
    text = fs.readFileSync(file, 'utf8');
    if (text.startsWith('bplist')) text = execFileSync('plutil', ['-convert', 'xml1', '-o', '-', file], { encoding: 'utf8', timeout: 5000 });
  } catch {
    return {};
  }
  const un = (v) => v.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, '&');
  const label = /<key>Label<\/key>\s*<string>([^<]*)<\/string>/.exec(text)?.[1];
  const arr = /<key>ProgramArguments<\/key>\s*<array>([\s\S]*?)<\/array>/.exec(text)?.[1];
  const args = arr === undefined ? null : [...arr.matchAll(/<string>([^<]*)<\/string>/g)].map((m) => un(m[1]));
  return { ...(label ? { label: un(label) } : {}), args };
}
