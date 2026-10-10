import zlib from 'node:zlib';

/**
 * A small PNG decoder for tests (8-bit RGB/RGBA/palette, non-interlaced), so the horn
 * mask can be checked on the real pet sheet without a browser. Returns {width, height,
 * data: Uint8ClampedArray RGBA}.
 */
export function decodePng(buf) {
  if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error('not a PNG');
  let pos = 8;
  let width, height, depth, type, interlace;
  let palette = null;
  let trns = null;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const kind = buf.toString('latin1', pos + 4, pos + 8);
    const body = buf.subarray(pos + 8, pos + 8 + len);
    if (kind === 'IHDR') {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      depth = body[8];
      type = body[9];
      interlace = body[12];
    } else if (kind === 'PLTE') palette = body;
    else if (kind === 'tRNS') trns = body;
    else if (kind === 'IDAT') idat.push(body);
    else if (kind === 'IEND') break;
    pos += 12 + len;
  }
  if (depth !== 8 || interlace !== 0) throw new Error(`unsupported PNG (depth ${depth}, interlace ${interlace})`);
  const channels = { 2: 3, 6: 4, 3: 1, 0: 1, 4: 2 }[type];
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const out = new Uint8ClampedArray(width * height * 4);
  let prev = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = Uint8Array.from(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? line[i - channels] : 0;
      const b = prev[i];
      const c = i >= channels ? prev[i - channels] : 0;
      const p = a + b - c;
      const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
      const paeth = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      line[i] = (line[i] + [0, a, b, (a + b) >> 1, paeth][filter]) & 255;
    }
    for (let x = 0; x < width; x++) {
      const o = (y * width + x) * 4;
      const s = x * channels;
      if (type === 6) out.set(line.subarray(s, s + 4), o);
      else if (type === 2) out.set([line[s], line[s + 1], line[s + 2], 255], o);
      else if (type === 3) out.set([palette[line[s] * 3], palette[line[s] * 3 + 1], palette[line[s] * 3 + 2], trns && line[s] < trns.length ? trns[line[s]] : 255], o);
      else if (type === 0) out.set([line[s], line[s], line[s], 255], o);
      else out.set([line[s], line[s], line[s], line[s + 1]], o);
    }
    prev = line;
  }
  return { width, height, data: out };
}

/** The pet sheet inside an animations.xml (base64 PNG, possibly in CDATA). */
export function petSheet(xml) {
  const b64 = /<png>([\s\S]*?)<\/png>/.exec(xml)[1].replace('<![CDATA[', '').replace(']]>', '').replace(/\s+/g, '');
  return decodePng(Buffer.from(b64, 'base64'));
}
