// DJ profile photos: shrunk to a small square on this computer, then sent to the
// other DJ over the booth link in pieces (each control message is one UDP packet,
// so it has to stay small) and put back together there.

import danaPhoto from '../assets/mock/dana.jpg';
import valPhoto from '../assets/mock/val.jpg';

/**
 * Until a DJ picks their own photo, the test DJs get the mockup photos (Brian,
 * 2026-10-03): Glizzy gets Dana's, Julio (and the Robot DJ, which stands in for
 * him) the sunglasses one. Both apps ship them, so nothing is sent.
 */
export function mockupPhoto(name: string): string | undefined {
  const n = name.trim().toLowerCase();
  if (n === 'glizzy') return danaPhoto;
  if (n === 'julio' || n === 'brians tester' || n === "brian's tester" || n === 'robot dj') return valPhoto;
  return undefined;
}

/** The photo to show for a DJ: their own if they picked one, else a mockup one. */
export function photoFor(dj: { name: string; photoUrl?: string } | null | undefined): string | undefined {
  return dj ? (dj.photoUrl ?? mockupPhoto(dj.name)) : undefined;
}

/** Side of the square photo we keep and send, in pixels. */
const PHOTO_PX = 240;
/** Characters of the data URL per message, well under the link's 1200-byte limit. */
export const PHOTO_CHUNK = 900;
/** Bigger photos are refused rather than sent (about 36 KB). */
export const MAX_PHOTO_CHUNKS = 40;
const ALLOWED = /^data:image\/(jpeg|png|webp|svg\+xml)[;,]/;

export interface PhotoChunk {
  t: 'photo';
  /** Same for every piece of one photo, so a newer photo replaces an older one. */
  id: string;
  i: number;
  n: number;
  d: string;
}

/** Center-crops a picked image file to a small square JPEG data URL. */
export async function shrinkPhoto(file: Blob): Promise<string> {
  const url = URL.createObjectURL(file);
  try {
    const img = await new Promise<HTMLImageElement>((resolve, reject) => {
      const i = new Image();
      i.onload = () => resolve(i);
      i.onerror = () => reject(new Error("That file isn't a picture we can read."));
      i.src = url;
    });
    const side = Math.min(img.naturalWidth, img.naturalHeight);
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = PHOTO_PX;
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error("Couldn't read that picture.");
    // Faces sit in the upper middle of most portraits.
    const sx = (img.naturalWidth - side) / 2;
    const sy = Math.max(0, (img.naturalHeight - side) * 0.3);
    ctx.drawImage(img, sx, sy, side, side, 0, 0, PHOTO_PX, PHOTO_PX);
    return canvas.toDataURL('image/jpeg', 0.72);
  } finally {
    URL.revokeObjectURL(url);
  }
}

/** A short fingerprint of the photo, used as its id. */
export function photoId(dataUrl: string): string {
  let h = 2166136261;
  for (let i = 0; i < dataUrl.length; i++) {
    h ^= dataUrl.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return (h >>> 0).toString(36) + dataUrl.length.toString(36);
}

export function photoChunks(dataUrl: string): PhotoChunk[] {
  const id = photoId(dataUrl);
  const n = Math.ceil(dataUrl.length / PHOTO_CHUNK);
  if (n > MAX_PHOTO_CHUNKS || !ALLOWED.test(dataUrl)) return [];
  return Array.from({ length: n }, (_, i) => ({ t: 'photo', id, i, n, d: dataUrl.slice(i * PHOTO_CHUNK, (i + 1) * PHOTO_CHUNK) }));
}

/** Puts the other DJ's photo back together; pieces may arrive twice or out of order. */
export class PhotoAssembler {
  private id: string | null = null;
  private parts: (string | undefined)[] = [];
  private done: string | null = null;

  /** Returns the whole photo the first time it is complete, otherwise null. */
  add(c: PhotoChunk): string | null {
    if (c.id === this.done) return null;
    if (c.id !== this.id) {
      this.id = c.id;
      this.parts = Array.from({ length: c.n }, () => undefined);
    }
    if (c.n !== this.parts.length || c.i >= c.n) return null;
    this.parts[c.i] = c.d;
    if (this.parts.some((p) => p === undefined)) return null;
    const url = this.parts.join('');
    this.done = c.id;
    this.id = null;
    this.parts = [];
    return ALLOWED.test(url) ? url : null;
  }
}
