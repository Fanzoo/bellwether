import { CliError, EXIT } from './util.mjs';

/**
 * Redaction and truncation for the only free text the brain keeps: tool summaries
 * (for example a Bash command) and notification or elicitation messages.
 *
 * Order matters: redact the whole string first, then truncate, so a secret cut in
 * half at the truncation point can't leave a recognisable prefix behind.
 */

export const REDACTED = '•••';

// C0/C1 control characters and the Unicode line and paragraph separators become a
// space. Zero-width and format characters (U+200B–200D, U+2060, U+FEFF) and the bidi
// marks (U+200E/200F, U+202A–202E, U+2066–2069) are removed outright: they could make
// a bubble read differently from what it contains, or split a word (pass<ZWSP>word)
// so a pattern misses it.
const CONTROL = new RegExp('[\\u0000-\\u001f\\u007f-\\u009f\\u2028\\u2029]+', 'g');
const INVISIBLE = new RegExp('[\\u200b-\\u200f\\u202a-\\u202e\\u2060-\\u2064\\u2066-\\u2069\\ufeff\\u00ad]', 'g');

/**
 * Text as a bubble may show it: NFKC (so ＰＡＳＳＷＯＲＤ folds to PASSWORD before any
 * pattern looks at it), invisible characters removed, control characters and runs of
 * whitespace collapsed to single spaces.
 */
export function normaliseText(text) {
  return text.normalize('NFKC').replace(INVISIBLE, '').replace(CONTROL, ' ').replace(/\s+/g, ' ').trim();
}

/** A sheep's label (a folder name): cleaned like bubble text, no redaction, at most `max`. */
export function cleanLabel(text, max = 200) {
  if (typeof text !== 'string') return undefined;
  const s = truncate(normaliseText(text), max);
  return s === '' ? undefined : s;
}

// ------------------------------------------------------------ secret names

// A name is secret when one of its components (split on _ - . and at camelCase
// boundaries) is one of these words, or ends in one of the longer ones: GITHUB_TOKEN,
// DB_PASSWORD, PGPASSWORD, AWS_SECRET_ACCESS_KEY, apiKey, _authToken, client_secret,
// ?sig=, ?key=. Whole components only, so `author`, `tokenizer` and `PASSWORDS` are
// not secret names.
const EXACT_WORDS = new Set(['key', 'pass', 'sig', 'auth', 'pwd', 'pin']);
const SUFFIX_WORDS = ['token', 'secret', 'password', 'passwd', 'passphrase', 'apikey', 'credential', 'credentials', 'signature', 'privatekey', 'secretkey', 'accesskey', 'sessionkey', 'authkey'];

export function isSecretName(name) {
  const parts = name
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .split(/[\s_.\-/:]+/)
    .map((p) => p.toLowerCase())
    .filter(Boolean);
  return parts.some((p) => EXACT_WORDS.has(p) || SUFFIX_WORDS.some((w) => p === w || p.endsWith(w)));
}

/** Values too plain to be a secret: short numbers, booleans, empty. Left readable. */
function isPlainValue(v) {
  const bare = v.replace(/^\\?["']|\\?["']$/g, '');
  return bare === '' || bare === REDACTED || /^(?:\d{1,6}|true|false|yes|no|on|off|null|none|nil|-)$/i.test(bare);
}

// A value: an escaped-quoted string (\"…\" inside a shell string), a quoted string
// (spaces included), or a run up to whitespace, a quote or a JSON/query/shell delimiter.
// Bounded (200) so a rejected match is cheap to rescan; a secret longer than that is
// still redacted to its end (see scanReplace).
const VALUE = `(?:\\\\"[^"\\\\\\n]{0,200}\\\\"|"[^"\\n]{0,200}"|'[^'\\n]{0,200}'|[^\\s"'\`,;&|}<>)\\\\]{1,200})`;
const VALUE_REST = /[^\s"'`,;&|}<>)\\]*/y;

const keyValue = (name, sep, value) => (isSecretName(name) && !isPlainValue(value) ? `${name}${sep}${REDACTED}` : null);
const flagValue = (flag, sep, value) => (isSecretName(flag) && !isPlainValue(value) && !value.startsWith('-') ? `${flag}${sep}${REDACTED}` : null);

/**
 * Like String.replace with a function, except that when `decide` returns null the
 * match is not consumed: scanning resumes one character later. A non-secret key
 * (`https:`, `--batch`) must not swallow a secret that follows inside its value
 * (`?client_secret=…`, `--passphrase …`).
 */
function scanReplace(re, decide) {
  return (text) => {
    let out = '';
    let last = 0;
    re.lastIndex = 0;
    let m;
    while ((m = re.exec(text)) !== null) {
      const rep = decide(...m.slice(1));
      if (rep === null) {
        re.lastIndex = m.index + 1;
        continue;
      }
      out += text.slice(last, m.index) + rep;
      last = m.index + m[0].length;
      // An unquoted value cut at the bound: redact the rest of it too.
      VALUE_REST.lastIndex = last;
      if (VALUE_REST.exec(text)) last = VALUE_REST.lastIndex;
      re.lastIndex = last;
    }
    return out + text.slice(last);
  };
}

/**
 * Bare base64 credentials are best effort: a standalone run of base64 characters is
 * redacted only when it looks like encoded bytes rather than a word, a path or a
 * hash: 16+ characters, upper and lower case and digits, not hex, padded with = or
 * 32+ long, and at least 3.5 bits of entropy per character. Hex hashes, UUIDs,
 * paths and identifiers are left alone; a short or low-entropy encoded secret can
 * still get through.
 */
function looksEncoded(tok) {
  if (tok.length > 4096 || /^[0-9a-f]+$/i.test(tok.replace(/=+$/, ''))) return false;
  if (!(/[A-Z]/.test(tok) && /[a-z]/.test(tok) && /[0-9]/.test(tok))) return false;
  if (!(tok.endsWith('=') || tok.length >= 32)) return false;
  if ((tok.match(/\//g) ?? []).length > 1) return false;
  const counts = {};
  for (const c of tok) counts[c] = (counts[c] ?? 0) + 1;
  let h = 0;
  for (const n of Object.values(counts)) h -= (n / tok.length) * Math.log2(n / tok.length);
  return h >= 3.5;
}

/**
 * The patterns always applied, before `bubble.redactPatterns`. Each is a regular
 * expression and a replacement (a string or a function). The name, flag or scheme
 * stays readable and only the secret becomes •••. They are generic shapes plus a
 * handful of well-known credential tools, they err on the side of redacting, and
 * their quantifiers are bounded so a pathological 5 MB input still cleans in linear
 * time.
 */
export const BUILTIN = [
  // Authorization: Basic|Bearer|… <credentials> (or = for env vars); the scheme stays.
  [/(authorization\\?["']?\s{0,20}[:=]\s{0,20}(?:(?:basic|bearer|digest|token|negotiate|ntlm|aws4-hmac-sha256)\s{1,20})?)[^\s"',;\\]+/gi, '$1•••'],
  [/(\b(?:bearer|basic)\s{1,20})[A-Za-z0-9._~+/=-]{6,}/gi, '$1•••'],
  // Cookie: / Set-Cookie: — the whole value list.
  [/(\b(?:set-)?cookie\\?["']?\s{0,20}[:=]\s{0,20})[^"'\n\\]{1,2000}/gi, '$1•••'],
  // name=value, name: value, "name":"value", \"name\":\"value\", ?name=value&
  [null, scanReplace(new RegExp(`(?<![A-Za-z0-9_.-])([A-Za-z0-9_.-]{1,80})(\\\\?["']?\\s{0,20}[=:]\\s{0,20})(${VALUE})`, 'g'), keyValue)],
  // --name value, --name=value, -name value: a flag whose name is secret
  [null, scanReplace(new RegExp(`(?<=^|\\s)(--?[A-Za-z][A-Za-z0-9_-]{0,60})(\\s{1,20}|=)(${VALUE})`, 'g'), flagValue)],
  // curl -u user:pass, -uuser:pass, --user=user:pass
  [/((?:^|\s)(?:-u|--user)(?:\s{1,20}|=)?["']?[^\s:"']{1,200}:)[^\s"']+/g, '$1•••'],
  // -pSECRET glued (mysql): 6+ characters that aren't a port mapping
  [/((?:^|\s)-p)(?![\d:.,/-]+(?:\s|$))[^\s"']{6,}/g, '$1•••'],
  // -p SECRET with a space, only for tools where -p means password
  [/(\b(?:docker|podman|nerdctl|buildah|skopeo|helm|oras)\s+(?:registry\s+)?login\b[^|;&\n]{0,300}?\s-p\s{1,20})(?!-)[^\s"']+/g, '$1•••'],
  [/(\b(?:mysql|mysqldump|mysqladmin|mysqlimport|mysqlshow|mariadb|mariadb-dump|sshpass)\b[^|;&\n]{0,300}?\s-p\s{1,20})(?!-)[^\s"']+/g, '$1•••'],
  // openssl -pass pass:… / -passin / -passout
  [/((?:^|\s)-pass(?:in|out|word)?\s{1,20}(?:pass:)?)[^\s"']+/g, '$1•••'],
  // aws configure set <secret-name> <value>
  [/(\baws\s+configure\s+set\s+)([A-Za-z0-9_.-]{1,80})(\s{1,20})(\S+)/g, (m, lead, name, sep, value) => (isSecretName(name) ? `${lead}${name}${sep}${REDACTED}` : m)],
  // vault login <token>
  [/(\bvault\s+login\s{1,20})(?!-)\S+/g, '$1•••'],
  // user:password@ in a URL
  [/(:\/\/[^/\s:@]{0,200}:)[^/\s@]{1,500}(?=@)/g, '$1•••'],
  // Token formats recognisable on their own.
  [/\b(?:sk|pk|rk)[-_](?:live_|test_)?[A-Za-z0-9_-]{16,}/g, '•••'],
  [/\bxox[abposr]-[A-Za-z0-9-]+/g, '•••'],
  [/\bgh[pousr]_[A-Za-z0-9]{20,}/g, '•••'],
  [/\bglpat-[A-Za-z0-9_-]{16,}/g, '•••'],
  [/\bAIza[0-9A-Za-z_-]{30,}/g, '•••'],
  [/\bAKIA[0-9A-Z]{16}\b/g, '•••'],
  [/\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]+/g, '•••'],
  // Bare base64-encoded bytes (best effort, see looksEncoded).
  [/(?<![A-Za-z0-9+/=_.-])[A-Za-z0-9+/]{16,}={0,2}(?![A-Za-z0-9+/=_.-])/g, (m) => (looksEncoded(m) ? REDACTED : m)],
].map(([re, replacement]) => ({ re, replacement }));

/** For tests and docs: the built-in patterns' sources (the two scanning rules have none). */
export const BUILTIN_PATTERNS = BUILTIN.filter(({ re }) => re).map(({ re }) => re.source);

/**
 * Compiles patterns written in the config's dialect. JavaScript has no inline
 * `(?i)` flag, so a leading `(?i)` becomes the `i` flag. Throws a config error
 * (exit 3) naming a pattern that doesn't compile.
 */
export function compilePatterns(patterns) {
  return patterns.map((p) => {
    if (typeof p !== 'string' || p === '') throw new CliError(`bubble.redactPatterns: every entry must be a non-empty string`, EXIT.BAD_CONFIG);
    const insensitive = p.startsWith('(?i)');
    try {
      return new RegExp(insensitive ? p.slice(4) : p, insensitive ? 'gi' : 'g');
    } catch (e) {
      throw new CliError(`bubble.redactPatterns: ${JSON.stringify(p)} is not a valid regular expression (${e.message})`, EXIT.BAD_CONFIG);
    }
  });
}

export function makeTextCleaner({ maxChars = 60, redactPatterns = [] } = {}) {
  const regexes = [...BUILTIN, ...compilePatterns(redactPatterns).map((re) => ({ re, replacement: REDACTED }))];
  return function clean(text) {
    if (typeof text !== 'string') return undefined;
    // Normalised first (NFKC, invisible characters out, whitespace collapsed), so one
    // bubble line can't be stretched or spoofed and a disguised name can't slip past.
    let s = normaliseText(text);
    for (const { re, replacement } of regexes) s = re ? s.replace(re, replacement) : replacement(s);
    return truncate(s, maxChars);
  };
}

/** Truncates to at most `max` characters (code points), ending with an ellipsis. */
export function truncate(s, max) {
  const chars = [...s];
  if (chars.length <= max) return s;
  return chars.slice(0, Math.max(0, max - 1)).join('') + '…';
}
