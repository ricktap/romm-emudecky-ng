/**
 * Steam derives a non-Steam shortcut's app id from its executable and name: `crc32(exe + name)` with the top bit set.
 * CRC32 is linear, so for a known prefix a short printable suffix can be solved that makes the CRC hit any target. That
 * is how a game gets the SAME id on every device (whatever `$HOME` is) without writing shortcuts.vdf: create the shortcut
 * under a name that carries the solved suffix, then rename it.
 */

const TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

const utf8 = (s: string): Uint8Array => new TextEncoder().encode(s);

/** The CRC register after `bytes`, continuing from `register` (initially 0xFFFFFFFF). Not final-xored. */
function advance(register: number, bytes: ArrayLike<number>): number {
  let r = register >>> 0;
  for (let i = 0; i < bytes.length; i++) r = (TABLE[(r ^ bytes[i]!) & 0xff]! ^ (r >>> 8)) >>> 0;
  return r;
}

export function crc32(input: string | Uint8Array): number {
  const bytes = typeof input === "string" ? utf8(input) : input;
  return (advance(0xffffffff, bytes) ^ 0xffffffff) >>> 0;
}

/** The app id Steam gives a shortcut created from `exe` and `name`: the CRC with the top bit set (unsigned 32-bit). */
export function steamShortcutId(exe: string, name: string): number {
  return (crc32(exe + name) | 0x80000000) >>> 0;
}

/** The same for the form Steam uses when the executable is quoted (as in shortcuts.vdf). */
export function steamShortcutIdQuoted(exe: string, name: string): number {
  return (crc32(`"${exe}"` + name) | 0x80000000) >>> 0;
}

/** A stable 32-bit id (top bit set) for a game: the CRC of `romm:<id>`. */
export function stableGameId(gameId: number | string): number {
  return (crc32(`romm:${gameId}`) | 0x80000000) >>> 0;
}

const SUFFIX_LENGTH = 7; // 7 bytes with 5 free bits each = 35 unknowns for 32 equations
const SUFFIX_BASE = 0x40; // bytes 0x40..0x5F: @ A-Z [ \ ] ^ _ (printable, valid UTF-8, no escaping needed)

/**
 * A suffix of printable ASCII such that `crc32(prefix + suffix) === targetCrc`, found by solving the linear system the
 * CRC defines over GF(2). Returns `null` in the (astronomically unlikely) case the system has no solution.
 */
export function solveSuffix(prefix: string, targetCrc: number): string | null {
  const start = advance(0xffffffff, utf8(prefix));
  const wanted = ((targetCrc ^ 0xffffffff) >>> 0);
  const encode = (bits: number[]): number[] => {
    const out: number[] = [];
    for (let i = 0; i < SUFFIX_LENGTH; i++) {
      let b = SUFFIX_BASE;
      for (let j = 0; j < 5; j++) if (bits[i * 5 + j]) b |= 1 << j;
      out.push(b);
    }
    return out;
  };
  const unknowns = SUFFIX_LENGTH * 5;
  const zero = new Array<number>(unknowns).fill(0);
  const base = advance(start, encode(zero));
  // Column j: the change of the final register when free bit j is set (the update is affine, so this is independent of the rest).
  const columns: number[] = [];
  for (let j = 0; j < unknowns; j++) {
    const bits = zero.slice();
    bits[j] = 1;
    columns.push((advance(start, encode(bits)) ^ base) >>> 0);
  }
  // Gaussian elimination: rows are the 32 output bits, columns the unknowns, augmented with the wanted difference.
  const target = (wanted ^ base) >>> 0;
  const rows: { mask: bigint; rhs: number }[] = [];
  for (let bit = 0; bit < 32; bit++) {
    let mask = 0n;
    for (let j = 0; j < unknowns; j++) if ((columns[j]! >>> bit) & 1) mask |= 1n << BigInt(j);
    rows.push({ mask, rhs: (target >>> bit) & 1 });
  }
  const pivotOf: number[] = [];
  let rank = 0;
  for (let col = 0; col < unknowns && rank < 32; col++) {
    const bitMask = 1n << BigInt(col);
    const pivot = rows.findIndex((r, i) => i >= rank && (r.mask & bitMask) !== 0n);
    if (pivot < 0) continue;
    [rows[rank], rows[pivot]] = [rows[pivot]!, rows[rank]!];
    for (let i = 0; i < 32; i++) {
      if (i !== rank && (rows[i]!.mask & bitMask) !== 0n) {
        rows[i]!.mask ^= rows[rank]!.mask;
        rows[i]!.rhs ^= rows[rank]!.rhs;
      }
    }
    pivotOf[rank] = col;
    rank++;
  }
  // Any row left with no unknowns but a 1 on the right means the target is unreachable.
  if (rows.slice(rank).some((r) => r.rhs !== 0)) return null;
  const solution = zero.slice();
  for (let i = 0; i < rank; i++) solution[pivotOf[i]!] = rows[i]!.rhs;
  const suffix = String.fromCharCode(...encode(solution));
  return crc32(prefix + suffix) === targetCrc >>> 0 ? suffix : null;
}

/** The name to create a shortcut under so that Steam assigns exactly `targetId` (top bit set) for `exe` + that name. */
export function forcedName(exe: string, baseName: string, targetId: number): string | null {
  const suffix = solveSuffix(exe + baseName, targetId >>> 0);
  return suffix === null ? null : baseName + suffix;
}
