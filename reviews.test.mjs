import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { reviewResponse } from './storage.mjs';

test('malformed manifest hashes cannot become review file paths', async () => {
  const root = await mkdtemp(resolve(import.meta.dirname, 'review-test-'));
  try {
    await mkdir(resolve(root, 'candidate'));
    await writeFile(resolve(root, 'candidate/manifest.json'), JSON.stringify([{ sha256: '../outside', name: 'candidate' }]));
    const response = await reviewResponse(new Request('https://evidence.linalab.io/api/reviews?slug=candidate'), root);
    assert.equal(response.status, 404);
    assert.equal((await response.json()).error, 'review manifest not found');
  } finally { await rm(root, { recursive: true }); }
});

test('server verdict persists and is readable without browser storage', async () => {
  const root = await mkdtemp(resolve(import.meta.dirname, 'review-test-'));
  try {
    await mkdir(resolve(root, 'candidate'));
    const hash = 'a'.repeat(64);
    await writeFile(resolve(root, 'candidate/manifest.json'), JSON.stringify([{ sha256: hash, name: '신종목 · 초상' }]));
    const url = 'https://evidence.linalab.io/api/reviews?slug=candidate';
    const response = await reviewResponse(new Request(url, { method: 'POST', headers: { origin: 'https://evidence.linalab.io' }, body: JSON.stringify({ sha256: hash, verdict: 'pass', note: '얼굴 확인' }) }), root);
    assert.equal(response.status, 200);
    assert.equal((await response.json()).saved, true);
    const saved = await (await reviewResponse(new Request(url), root)).json();
    assert.equal(saved.decisions[hash].verdict, 'pass');
    assert.equal(saved.decisions[hash].note, '얼굴 확인');
    const foreign = await reviewResponse(new Request(url, { method: 'POST', headers: { origin: 'https://other.invalid' }, body: '{}' }), root);
    assert.equal(foreign.status, 403);
    const invalid = await reviewResponse(new Request(url, { method: 'POST', headers: { origin: 'https://evidence.linalab.io' }, body: JSON.stringify({ sha256: 'b'.repeat(64), verdict: 'pass', note: '' }) }), root);
    assert.equal(invalid.status, 400);
    const proxied = await reviewResponse(new Request('http://internal/api/reviews?slug=candidate', { method: 'POST', headers: { origin: 'https://evidence.linalab.io' }, body: JSON.stringify({ sha256: hash, verdict: 'fail', note: '소품 수정' }) }), root, 'https://evidence.linalab.io');
    assert.equal(proxied.status, 200);
  } finally { await rm(root, { recursive: true }); }
});
