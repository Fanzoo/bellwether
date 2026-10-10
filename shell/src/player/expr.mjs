/**
 * The safe evaluator for the arithmetic in pet XML (`<x>`, `<y>`, `repeat="…"`).
 *
 * Pet files come from a third-party repository, so nothing in them is ever run as
 * code: there is no eval and no Function, and the webview's CSP forbids
 * 'unsafe-eval' anyway. The grammar is numbers, + - * / and parentheses, unary
 * minus, and a fixed set of keywords:
 *
 *   screenW screenH areaW areaH imageW imageH imageX imageY random randS
 *
 * Anything else (another identifier, a quote, a dot after a name, a call) is an
 * error: `evaluate` returns 0 and reports a warning once per expression.
 */

export const KEYWORDS = ['screenW', 'screenH', 'areaW', 'areaH', 'imageW', 'imageH', 'imageX', 'imageY', 'random', 'randS'];
const MAX_LENGTH = 200;

export class ExpressionError extends Error {}

/** Parses once into a small tree; `run(tree, env)` evaluates it. */
export function compile(source) {
  const text = String(source ?? '').trim();
  if (text.length > MAX_LENGTH) throw new ExpressionError('expression too long');
  const tokens = tokenize(text);
  let i = 0;
  const peek = () => tokens[i];
  const take = () => tokens[i++];

  function expression() {
    let left = term();
    while (peek() === '+' || peek() === '-') left = { op: take(), left, right: term() };
    return left;
  }
  function term() {
    let left = unary();
    while (peek() === '*' || peek() === '/') left = { op: take(), left, right: unary() };
    return left;
  }
  function unary() {
    if (peek() === '-') {
      take();
      return { op: 'neg', arg: unary() };
    }
    if (peek() === '+') {
      take();
      return unary();
    }
    return primary();
  }
  function primary() {
    const t = take();
    if (t === undefined) throw new ExpressionError('unexpected end');
    if (t === '(') {
      const e = expression();
      if (take() !== ')') throw new ExpressionError('missing )');
      return e;
    }
    if (typeof t === 'number') return { num: t };
    if (typeof t === 'object' && t.name) return { name: t.name };
    throw new ExpressionError(`unexpected ${JSON.stringify(t)}`);
  }

  if (tokens.length === 0) throw new ExpressionError('empty expression');
  const tree = expression();
  if (i !== tokens.length) throw new ExpressionError(`unexpected ${JSON.stringify(tokens[i])}`);
  return tree;
}

function tokenize(text) {
  const out = [];
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === ' ' || c === '\t' || c === '\n' || c === '\r') {
      i++;
    } else if ('+-*/()'.includes(c)) {
      out.push(c);
      i++;
    } else if ((c >= '0' && c <= '9') || c === '.') {
      let j = i;
      while (j < text.length && /[0-9.]/.test(text[j])) j++;
      const n = Number(text.slice(i, j));
      if (!Number.isFinite(n)) throw new ExpressionError(`bad number ${text.slice(i, j)}`);
      out.push(n);
      i = j;
    } else if (/[A-Za-z_]/.test(c)) {
      let j = i;
      while (j < text.length && /[A-Za-z0-9_]/.test(text[j])) j++;
      const name = text.slice(i, j);
      if (!KEYWORDS.includes(name)) throw new ExpressionError(`unknown name ${name}`);
      out.push({ name });
      i = j;
    } else {
      throw new ExpressionError(`unexpected character ${JSON.stringify(c)}`);
    }
  }
  return out;
}

export function run(tree, env) {
  if ('num' in tree) return tree.num;
  if ('name' in tree) {
    const v = env[tree.name];
    return typeof v === 'number' && Number.isFinite(v) ? v : 0;
  }
  if (tree.op === 'neg') return -run(tree.arg, env);
  const a = run(tree.left, env);
  const b = run(tree.right, env);
  switch (tree.op) {
    case '+':
      return a + b;
    case '-':
      return a - b;
    case '*':
      return a * b;
    case '/':
      return b === 0 ? 0 : a / b;
    default:
      return 0;
  }
}

const cache = new Map();
const warned = new Set();

/**
 * Evaluates an expression string. Never throws: a malformed or hostile expression
 * evaluates to 0, and `warn` is called once per distinct expression.
 */
export function evaluate(source, env, warn = () => {}) {
  const key = String(source ?? '');
  let tree = cache.get(key);
  if (tree === undefined) {
    try {
      tree = compile(key);
    } catch (e) {
      tree = null;
      if (!warned.has(key)) {
        warned.add(key);
        warn(`pet expression ${JSON.stringify(key.slice(0, 80))} rejected (${e.message}); using 0`);
      }
    }
    if (cache.size < 1000) cache.set(key, tree);
  }
  if (tree === null) return 0;
  const v = run(tree, env);
  return Number.isFinite(v) ? v : 0;
}
