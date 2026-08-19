// Generates a 1024x1024 RGBA PNG (brand placeholder: dark rounded tile with a
// light "D" arc) with zero dependencies, so `tauri icon` has a source image.
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const SIZE = 1024;

const CRC_TABLE = new Int32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  CRC_TABLE[n] = c;
}

function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function pixel(x, y) {
  // Background: near-black with a soft rounded-rect vignette.
  const margin = 96;
  const radius = 180;
  const inside =
    x >= margin &&
    x < SIZE - margin &&
    y >= margin &&
    y < SIZE - margin &&
    roundedRect(x - margin, y - margin, SIZE - 2 * margin, radius);
  if (!inside) return [0, 0, 0, 0];

  // Brand: an indigo circle ring with a lighter center dot.
  const cx = SIZE / 2;
  const cy = SIZE / 2;
  const dx = x - cx;
  const dy = y - cy;
  const dist = Math.sqrt(dx * dx + dy * dy);

  if (dist > 210) return [13, 13, 24, 255]; // dark tile
  if (dist > 180) return [77, 107, 254, 255]; // ring
  if (dist > 70) return [13, 13, 24, 255]; // inner dark
  return [232, 232, 240, 255]; // center dot
}

function roundedRect(x, y, size, radius) {
  const cx = Math.min(Math.max(x, radius), size - radius);
  const cy = Math.min(Math.max(y, radius), size - radius);
  const dx = x - cx;
  const dy = y - cy;
  return dx * dx + dy * dy <= radius * radius;
}

const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
for (let y = 0; y < SIZE; y++) {
  raw[y * (SIZE * 4 + 1)] = 0; // filter: none
  for (let x = 0; x < SIZE; x++) {
    const [r, g, b, a] = pixel(x, y);
    const off = y * (SIZE * 4 + 1) + 1 + x * 4;
    raw[off] = r;
    raw[off + 1] = g;
    raw[off + 2] = b;
    raw[off + 3] = a;
  }
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type: RGBA
ihdr[10] = 0; // compression
ihdr[11] = 0; // filter
ihdr[12] = 0; // interlace

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

const out = new URL("../src-tauri/icons/icon-source.png", import.meta.url);
writeFileSync(out, png);
console.log(`wrote ${out.pathname} (${png.length} bytes)`);
