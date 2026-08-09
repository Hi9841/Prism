/**
 * Safe math evaluation - a tiny recursive-descent parser.
 * No eval(), no global state. Handles + - * / ^ % ( ) and common
 * functions/constants. Returns null when the expression isn't math.
 */

export interface EvalResult {
  value: number;
  expr: string;
}

const FUNCS: Record<string, (x: number) => number> = {
  sqrt: Math.sqrt,
  abs: Math.abs,
  floor: Math.floor,
  ceil: Math.ceil,
  round: Math.round,
  ln: Math.log,
  log: Math.log10,
  sin: Math.sin,
  cos: Math.cos,
  tan: Math.tan,
  asin: Math.asin,
  acos: Math.acos,
  atan: Math.atan,
  exp: Math.exp,
};

const CONSTS: Record<string, number> = {
  pi: Math.PI,
  e: Math.E,
  tau: Math.PI * 2,
};

function normalize(input: string): string | null {
  let s = input.trim().toLowerCase();
  if (s.length === 0 || s.length > 120) return null;
  s = s.replace(/×/g, "*").replace(/÷/g, "/");
  s = s
    .replace(/√([\d.]+)/g, "sqrt($1)")
    .replace(/√/g, "sqrt(")
    .replace(/π/g, "pi");
  // Only allow the token set we can parse.
  if (!/^[\d\s.+\-*/^%()a-z]+$/.test(s)) return null;
  // Must contain a digit or a known constant (not just "sqrt()").
  if (!/\d/.test(s) && !/(^|[^a-z])(pi|e|tau)([^a-z]|$)/.test(s)) return null;
  return s;
}

type Token =
  | { t: "num"; v: number }
  | { t: "op"; v: string }
  | { t: "lparen" }
  | { t: "rparen" }
  | { t: "ident"; v: string };

class Parser {
  private pos = 0;

  constructor(private tokens: Token[]) {}

  private peek(): Token | undefined {
    return this.tokens[this.pos];
  }

  private next(): Token | undefined {
    return this.tokens[this.pos++];
  }

  parse(): number {
    const v = this.expr();
    if (this.pos !== this.tokens.length) throw new Error("trailing input");
    return v;
  }

  private expr(): number {
    let v = this.term();
    for (;;) {
      const t = this.peek();
      if (t?.t === "op" && (t.v === "+" || t.v === "-")) {
        this.next();
        const rhs = this.term();
        v = t.v === "+" ? v + rhs : v - rhs;
      } else {
        return v;
      }
    }
  }

  private term(): number {
    let v = this.factor();
    for (;;) {
      const t = this.peek();
      if (t?.t === "op" && (t.v === "*" || t.v === "/" || t.v === "%")) {
        this.next();
        const rhs = this.factor();
        if (t.v === "*") v = v * rhs;
        else if (t.v === "/") v = v / rhs;
        else v = v % rhs;
      } else {
        return v;
      }
    }
  }

  private factor(): number {
    const t = this.peek();
    if (t?.t === "op" && (t.v === "-" || t.v === "+")) {
      this.next();
      const v = this.factor();
      return t.v === "-" ? -v : v;
    }
    const v = this.primary();
    // Power binds tighter than everything: -2^2 == -(2^2)
    const t2 = this.peek();
    if (t2?.t === "op" && t2.v === "^") {
      this.next();
      const exp = this.factor();
      return v ** exp;
    }
    return v;
  }

  private primary(): number {
    const t = this.next();
    if (!t) throw new Error("unexpected end");
    switch (t.t) {
      case "num":
        return t.v;
      case "lparen": {
        const v = this.expr();
        const close = this.next();
        if (close?.t !== "rparen") throw new Error("expected )");
        return v;
      }
      case "ident": {
        if (t.v in CONSTS) return CONSTS[t.v];
        const fn = FUNCS[t.v];
        if (!fn) throw new Error("unknown ident");
        const lp = this.next();
        if (lp?.t !== "lparen") throw new Error("expected ( after function");
        const arg = this.expr();
        const rp = this.next();
        if (rp?.t !== "rparen") throw new Error("expected ) after function arg");
        return fn(arg);
      }
      default:
        throw new Error("unexpected token");
    }
  }
}

function tokenize(s: string): Token[] {
  const out: Token[] = [];
  let i = 0;
  while (i < s.length) {
    const c = s[i];
    if (c === " ") {
      i++;
      continue;
    }
    if (/[0-9.]/.test(c)) {
      let j = i;
      while (j < s.length && /[0-9.]/.test(s[j])) j++;
      const num = Number(s.slice(i, j));
      if (!Number.isFinite(num)) throw new Error("bad number");
      out.push({ t: "num", v: num });
      i = j;
      continue;
    }
    if (/[a-z]/.test(c)) {
      let j = i;
      while (j < s.length && /[a-z]/.test(s[j])) j++;
      out.push({ t: "ident", v: s.slice(i, j) });
      i = j;
      continue;
    }
    if ("+-*/^%".includes(c)) {
      out.push({ t: "op", v: c });
      i++;
      continue;
    }
    if (c === "(") {
      out.push({ t: "lparen" });
      i++;
      continue;
    }
    if (c === ")") {
      out.push({ t: "rparen" });
      i++;
      continue;
    }
    throw new Error("unexpected char");
  }
  return out;
}

/** Formats a result number: 6 significant digits, no trailing zeros. */
export function formatNumber(n: number): string {
  if (!Number.isFinite(n)) return "Undefined";
  if (Number.isInteger(n)) return String(n);
  const rounded = Number(n.toPrecision(6));
  return String(rounded);
}

/** Returns the evaluation result if `input` parses as a math expression. */
export function tryEvaluate(input: string): EvalResult | null {
  const s = normalize(input);
  if (!s) return null;
  try {
    const value = new Parser(tokenize(s)).parse();
    return { value, expr: s.replace(/\s+/g, "") };
  } catch {
    return null;
  }
}

/** Returns true when a query is "math-like" enough to surface the calculator. */
export function isMathLike(input: string): boolean {
  const s = input.trim().toLowerCase();
  if (s.length === 0 || s.length > 120) return false;
  if (!/\d/.test(s)) return false;
  return /^[\d\s.+\-*/^%()a-z]+$/.test(s);
}
