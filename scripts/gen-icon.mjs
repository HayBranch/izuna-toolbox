/**
 * 生成 1024x1024 的应用图标源图（RGBA PNG，零依赖）：
 * 泉奈工具箱 —— 忍者手里剑主题（靛蓝→天蓝渐变圆角方块 + 白色四刃手里剑，中心镂空）。
 * 之后交给 `tauri icon` 生成全平台图标：
 *   npm run icon
 */
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SIZE = 1024;

// ---------- 最小 PNG 编码器 ----------
const CRC_TABLE = new Int32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c;
});

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
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

function encodePng(pixels) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(SIZE, 0);
  ihdr.writeUInt32BE(SIZE, 4);
  ihdr[8] = 8; // 位深
  ihdr[9] = 6; // 颜色类型：RGBA

  // 每行前置 filter 字节 0
  const stride = SIZE * 4 + 1;
  const raw = Buffer.alloc(stride * SIZE);
  for (let y = 0; y < SIZE; y++) {
    raw[y * stride] = 0;
    Buffer.from(pixels.buffer, y * SIZE * 4, SIZE * 4).copy(raw, y * stride + 1);
  }

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ---------- 几何：带圆角矩形的符号距离 ----------
function sdRect(px, py, cx, cy, hw, hh, r) {
  const dx = Math.abs(px - cx) - (hw - r);
  const dy = Math.abs(py - cy) - (hh - r);
  const ox = Math.max(dx, 0);
  const oy = Math.max(dy, 0);
  return Math.min(Math.max(dx, dy), 0) + Math.hypot(ox, oy) - r;
}

// ---------- 几何：四刃手里剑（凹四角星）+ 中心圆孔，射线法判定内部 ----------
const STAR = [];
for (let i = 0; i < 8; i++) {
  const ang = -Math.PI / 2 + (i * Math.PI) / 4; // 从正上方开始，外刃/内凹交替
  const r = i % 2 === 0 ? 372 : 104;
  STAR.push([512 + Math.cos(ang) * r, 512 + Math.sin(ang) * r]);
}

function pointInPolygon(px, py, vs) {
  let inside = false;
  for (let i = 0, j = vs.length - 1; i < vs.length; j = i++) {
    const [xi, yi] = vs[i];
    const [xj, yj] = vs[j];
    if (yi > py !== yj > py && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi) {
      inside = !inside;
    }
  }
  return inside;
}

// ---------- 绘制：sky→indigo 渐变圆角方块 + 白色手里剑（3x3 超采样抗锯齿） ----------
const pixels = new Uint8Array(SIZE * SIZE * 4);
const C1 = [56, 189, 248]; // sky-400
const C2 = [99, 102, 241]; // indigo-500

for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    const i = (y * SIZE + x) * 4;

    // 大圆角方块背景（四边留 24px 透明边距）
    const bg = sdRect(x + 0.5, y + 0.5, SIZE / 2, SIZE / 2, SIZE / 2 - 24, SIZE / 2 - 24, 200);
    const bgAlpha = Math.min(Math.max(0.5 - bg, 0), 1);
    if (bgAlpha <= 0) continue;

    // 对角渐变
    const t = (x + y) / (2 * SIZE);
    let r = C1[0] + (C2[0] - C1[0]) * t;
    let g = C1[1] + (C2[1] - C1[1]) * t;
    let b = C1[2] + (C2[2] - C1[2]) * t;

    // 白色手里剑：3x3 超采样；中心圆孔（半径 88）透出背景渐变
    let hit = 0;
    for (let sy = 0; sy < 3; sy++) {
      for (let sx = 0; sx < 3; sx++) {
        const px = x + (sx + 0.5) / 3;
        const py = y + (sy + 0.5) / 3;
        const dist = Math.hypot(px - 512, py - 512);
        if (dist > 88 && pointInPolygon(px, py, STAR)) hit++;
      }
    }
    const fg = hit / 9;
    r += (255 - r) * fg;
    g += (255 - g) * fg;
    b += (255 - b) * fg;

    pixels[i] = Math.round(r);
    pixels[i + 1] = Math.round(g);
    pixels[i + 2] = Math.round(b);
    pixels[i + 3] = Math.round(bgAlpha * 255);
  }
}

const here = path.dirname(fileURLToPath(import.meta.url));
const out = path.join(here, "app-icon.png");
writeFileSync(out, encodePng(pixels));
console.log(`✅ 已生成图标源图：${out} (${SIZE}x${SIZE})`);
console.log("下一步：npx tauri icon scripts/app-icon.png");
