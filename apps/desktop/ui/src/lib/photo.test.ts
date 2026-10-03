import { describe, expect, it } from 'vitest';
import { parsePeerMessage } from '../session/peer';
import { PHOTO_CHUNK, PhotoAssembler, photoChunks } from './photo';

const photo = 'data:image/jpeg;base64,' + 'A'.repeat(PHOTO_CHUNK * 3 + 17);

describe('profile photos over the booth link', () => {
  it('splits a photo into pieces that each fit in one control message', () => {
    const chunks = photoChunks(photo);
    expect(chunks).toHaveLength(4);
    for (const c of chunks) expect(JSON.stringify(c).length).toBeLessThan(1200);
  });
  it('puts it back together from pieces that arrive twice and out of order', () => {
    const a = new PhotoAssembler();
    const [c0, c1, c2, c3] = photoChunks(photo).map((c) => parsePeerMessage(JSON.stringify(c)));
    for (const c of [c2, c0, c2, c3]) expect(a.add(c as never)).toBeNull();
    expect(a.add(c1 as never)).toBe(photo);
    // Resends of the same photo are ignored once it is shown.
    expect(a.add(c1 as never)).toBeNull();
  });
  it('refuses anything that is not a picture, and pieces that are too big', () => {
    expect(photoChunks('javascript:alert(1)')).toEqual([]);
    expect(parsePeerMessage(JSON.stringify({ t: 'photo', id: 'x', i: 0, n: 1, d: 'A'.repeat(PHOTO_CHUNK + 1) }))).toBeNull();
    expect(parsePeerMessage(JSON.stringify({ t: 'photo', id: 'x', i: 2, n: 2, d: 'A' }))).toBeNull();
    const a = new PhotoAssembler();
    expect(a.add({ t: 'photo', id: 'x', i: 0, n: 1, d: 'data:text/html,<b>hi</b>' })).toBeNull();
  });
});
