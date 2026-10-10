import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const AGENT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const FIXTURES = path.join(AGENT_DIR, 'test', 'fixtures');
export const TOKEN = 'testTokenNotReal_abcdefghijklmnop';

export function tempDir(prefix = 'pasture-test-') {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

/**
 * A raw HTTP request where every header is under the test's control (including
 * Host and Origin). Resolves with status, headers and the body as a Buffer.
 */
export function request(port, { method = 'POST', path: p = '/hook', headers = {}, body = '' } = {}) {
  return new Promise((resolve, reject) => {
    const data = Buffer.isBuffer(body) ? body : Buffer.from(body);
    const req = http.request(
      { host: '127.0.0.1', port, method, path: p, headers: { 'Content-Length': data.length, ...headers }, setHost: false },
      (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) }));
      },
    );
    req.on('error', reject);
    req.end(data);
  });
}

export function hookHeaders(port, extra = {}) {
  return { Host: `127.0.0.1:${port}`, 'Content-Type': 'application/json', 'X-Pasture-Token': TOKEN, ...extra };
}
