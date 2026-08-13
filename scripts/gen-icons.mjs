// 生成 Tauri 需要的图标：PNG + 多尺寸 ICO（BMP 编码，兼容性最好）
// 纯 Node 实现，不引第三方依赖。运行：node scripts/gen-icons.mjs
import { deflateSync } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const OUT = join(dirname(fileURLToPath(import.meta.url)), '..', 'src-tauri', 'icons');

// ---------- 画图 ----------
const BG = [0x24, 0x1f, 0x4d];
const NOTE = [0xf5, 0xc5, 0x42];
const DOT = [0xef, 0x44, 0x44];

function coverage(size, fn) {
  // 3x3 超采样，让小尺寸边缘不至于太糙
  let hit = 0;
  for (let sy = 0; sy < 3; sy++) {
    for (let sx = 0; sx < 3; sx++) {
      if (fn((sx + 0.5) / 3, (sy + 0.5) / 3)) hit++;
    }
  }
  return hit / 9;
}

function pixels(size) {
  const buf = Buffer.alloc(size * size * 4);
  const r = 0.22; // 圆角半径（相对 1.0）
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const at = (ox, oy) => {
        const u = (x + ox) / size;
        const v = (y + oy) / size;
        return { u, v };
      };
      const inRounded = (ox, oy) => {
        const { u, v } = at(ox, oy);
        const cx = Math.min(Math.max(u, r), 1 - r);
        const cy = Math.min(Math.max(v, r), 1 - r);
        return (u - cx) ** 2 + (v - cy) ** 2 <= r * r;
      };
      const inNote = (ox, oy) => {
        const { u, v } = at(ox, oy);
        return u >= 0.28 && u <= 0.72 && v >= 0.3 && v <= 0.74;
      };
      const inDot = (ox, oy) => {
        const { u, v } = at(ox, oy);
        return (u - 0.72) ** 2 + (v - 0.27) ** 2 <= 0.115 ** 2;
      };

      const aBg = coverage(size, inRounded);
      const aNote = coverage(size, inNote);
      const aDot = coverage(size, inDot);

      let col = BG.slice();
      if (aNote > 0) col = col.map((c, i) => Math.round(c * (1 - aNote) + NOTE[i] * aNote));
      if (aDot > 0) col = col.map((c, i) => Math.round(c * (1 - aDot) + DOT[i] * aDot));

      const o = (y * size + x) * 4;
      buf[o] = col[0];
      buf[o + 1] = col[1];
      buf[o + 2] = col[2];
      buf[o + 3] = Math.round(255 * aBg);
    }
  }
  return buf;
}

// ---------- PNG ----------
const CRC = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();

function crc32(b) {
  let c = ~0;
  for (const byte of b) c = CRC[(c ^ byte) & 0xff] ^ (c >>> 8);
  return ~c >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function png(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// ---------- ICO（BMP 条目，RC.exe 与老工具链都吃得下） ----------
function dib(size, rgba) {
  const header = Buffer.alloc(40);
  header.writeUInt32LE(40, 0);
  header.writeInt32LE(size, 4);
  header.writeInt32LE(size * 2, 8); // 高度 x2：颜色数据 + AND 掩码
  header.writeUInt16LE(1, 12);
  header.writeUInt16LE(32, 14);
  const px = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    const src = (size - 1 - y) * size * 4; // 自底向上
    for (let x = 0; x < size; x++) {
      const s = src + x * 4;
      const d = (y * size + x) * 4;
      px[d] = rgba[s + 2]; // B
      px[d + 1] = rgba[s + 1]; // G
      px[d + 2] = rgba[s]; // R
      px[d + 3] = rgba[s + 3]; // A
    }
  }
  const maskRow = Math.ceil(size / 32) * 4;
  return Buffer.concat([header, px, Buffer.alloc(maskRow * size)]);
}

function ico(images) {
  const count = images.length;
  const dir = Buffer.alloc(6 + count * 16);
  dir.writeUInt16LE(0, 0);
  dir.writeUInt16LE(1, 2);
  dir.writeUInt16LE(count, 4);
  let offset = dir.length;
  images.forEach((img, i) => {
    const e = 6 + i * 16;
    dir[e] = img.size >= 256 ? 0 : img.size;
    dir[e + 1] = img.size >= 256 ? 0 : img.size;
    dir.writeUInt16LE(1, e + 4);
    dir.writeUInt16LE(32, e + 6);
    dir.writeUInt32LE(img.data.length, e + 8);
    dir.writeUInt32LE(offset, e + 12);
    offset += img.data.length;
  });
  return Buffer.concat([dir, ...images.map((i) => i.data)]);
}

// ---------- 输出 ----------
mkdirSync(OUT, { recursive: true });

const cache = new Map();
const rgbaFor = (s) => {
  if (!cache.has(s)) cache.set(s, pixels(s));
  return cache.get(s);
};

for (const [name, size] of [
  ['32x32.png', 32],
  ['128x128.png', 128],
  ['128x128@2x.png', 256],
  ['icon.png', 512],
]) {
  writeFileSync(join(OUT, name), png(size, rgbaFor(size)));
  console.log('wrote', name);
}

writeFileSync(
  join(OUT, 'icon.ico'),
  ico([16, 32, 48, 64, 256].map((s) => ({ size: s, data: dib(s, rgbaFor(s)) }))),
);
console.log('wrote icon.ico');
