import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { cleanOverlays, overlayPlacements } from '../shell/src/player/overlay.mjs';
import { AGENT_DIR, isRealHome, tempDir } from './helpers.mjs';

/**
 * Tester probes for M10a (bundled pets and the sheep). Everything that writes works in a
 * temp copy of scripts/, pets/, pets.lock.json and the player; the repository's files are
 * only read. The Rust-binary probes run only when BELLWETHER_BIN names a binary, under a
 * temp HOME and BELLWETHER_HOME.
 */

const copyRepo = () => {
  // realpath: build-pet only runs main() when argv[1] equals its real path (finding: a symlinked path is a silent no-op that exits 0).
  const root = fs.realpathSync(tempDir('m10a-copy-'));
  fs.mkdirSync(path.join(root, 'shell', 'src'), { recursive: true });
  fs.cpSync(path.join(AGENT_DIR, 'scripts'), path.join(root, 'scripts'), { recursive: true });
  fs.cpSync(path.join(AGENT_DIR, 'pets'), path.join(root, 'pets'), { recursive: true });
  fs.cpSync(path.join(AGENT_DIR, 'shell', 'src', 'player'), path.join(root, 'shell', 'src', 'player'), { recursive: true });
  fs.copyFileSync(path.join(AGENT_DIR, 'pets.lock.json'), path.join(root, 'pets.lock.json'));
  return root;
};
const build = (root, ...args) => spawnSync(process.execPath, [...(args[0]?.startsWith('--import') ? [args.shift()] : []), path.join(root, 'scripts', 'build-pet.mjs'), ...args], { encoding: 'utf8', timeout: 120_000 });
const read = (root, rel) => fs.readFileSync(path.join(root, rel));

describe('build-pet: deterministic, and independent of node:zlib', () => {
  test('--check passes with node:zlib\'s deflate and gzip made to throw: the PNG is the script\'s own encoding', () => {
    const stub = path.join(tempDir('m10a-stub-'), 'stub.mjs');
    fs.writeFileSync(stub, "import zlib from 'node:zlib';\nfor (const k of ['deflateSync','deflateRawSync','gzipSync','deflate','deflateRaw','gzip']) zlib[k] = () => { throw new Error('node:zlib ' + k + ' used'); };\n");
    const r = build(AGENT_DIR, `--import=file://${stub}`, '--check');
    assert.equal(r.status, 0, r.stderr);
  });

  test('a rebuild in a clean copy is byte-identical for animations.xml and the pin file, twice', () => {
    const root = copyRepo();
    for (let i = 0; i < 2; i++) {
      const r = build(root, 'build', '--all');
      assert.equal(r.status, 0, r.stderr);
      assert.ok(read(root, 'pets/bellwether/animations.xml').equals(fs.readFileSync(path.join(AGENT_DIR, 'pets/bellwether/animations.xml'))));
      assert.ok(read(root, 'pets.lock.json').equals(fs.readFileSync(path.join(AGENT_DIR, 'pets.lock.json'))));
    }
  });
});

describe('build-pet --check catches an edit anywhere in the chain', () => {
  const edits = {
    'one pixel of a part': (root) => {
      const f = path.join(root, 'pets/bellwether/src/parts/leg.txt');
      fs.writeFileSync(f, fs.readFileSync(f, 'utf8').replace('FF', '.F'));
    },
    'a frame\'s offset': (root) => {
      const f = path.join(root, 'pets/bellwether/src/frames/walk_01.txt');
      fs.writeFileSync(f, fs.readFileSync(f, 'utf8').replace('part body 11 19', 'part body 11 20'));
    },
    'an interval in pet.json': (root) => {
      const f = path.join(root, 'pets/bellwether/pet.json');
      fs.writeFileSync(f, fs.readFileSync(f, 'utf8').replace('"interval": 140', '"interval": 141'));
    },
    'an overlay position in pet.json': (root) => {
      const f = path.join(root, 'pets/bellwether/pet.json');
      fs.writeFileSync(f, fs.readFileSync(f, 'utf8').replace('"x": 9, "y": 12', '"x": 10, "y": 12'));
    },
    'one appended byte of animations.xml': (root) => fs.appendFileSync(path.join(root, 'pets/bellwether/animations.xml'), '\n'),
    'a pinned overlay tile': (root) => {
      const f = path.join(root, 'pets.lock.json');
      fs.writeFileSync(f, fs.readFileSync(f, 'utf8').replace('"tile": 53', '"tile": 54'));
    },
    'a pinned sha256 digit': (root) => {
      const f = path.join(root, 'pets.lock.json');
      fs.writeFileSync(f, fs.readFileSync(f, 'utf8').replace('11886b08', '11886b09'));
    },
    'the bundled key removed from the lock': (root) => {
      const f = path.join(root, 'pets.lock.json');
      const j = JSON.parse(fs.readFileSync(f, 'utf8'));
      delete j.bundled;
      fs.writeFileSync(f, JSON.stringify(j, null, 2) + '\n');
    },
  };
  for (const [what, edit] of Object.entries(edits)) {
    test(what, () => {
      const root = copyRepo();
      assert.equal(build(root, '--check').status, 0, 'the copy starts clean');
      edit(root);
      const r = build(root, '--check');
      assert.equal(r.status, 1, `${r.stdout}${r.stderr}`);
      assert.match(r.stderr, /build-pet --check: /);
    });
  }
});

describe('build-pet on hostile sources: a reason, never a stack trace, never a write', () => {
  const hostile = {
    'a part name that climbs out of parts/': 'part ../../../../etc/hosts 0 0\n',
    '__proto__ as a part': 'part __proto__ 0 0\n',
    'constructor as a part': 'part constructor 0 0\n',
    'a 2000-row, 5000-column grid': Array(2000).fill('F'.repeat(5000)).join('\n') + '\n',
    'binary junk in a grid': Buffer.from('grid 0 0\n\xff\xfe\x00AA\nend\n', 'latin1'),
    'an offset that is not an integer': 'part leg 1e999 NaN\n',
    'an unclosed grid': 'grid 0 0\nAAA\n',
  };
  for (const [what, text] of Object.entries(hostile)) {
    test(what, () => {
      const root = copyRepo();
      fs.writeFileSync(path.join(root, 'pets/bellwether/src/frames/walk_01.txt'), text);
      const before = read(root, 'pets/bellwether/animations.xml');
      for (const args of [['--check'], ['build', 'bellwether']]) {
        const r = build(root, ...args);
        assert.equal(r.status, 1, `${args}: ${r.stdout}${r.stderr}`);
        assert.match(r.stderr, /^build-pet: /m);
        assert.doesNotMatch(r.stderr, /\n\s+at /, 'no stack trace');
      }
      assert.ok(read(root, 'pets/bellwether/animations.xml').equals(before), 'a refused build writes nothing');
    });
  }
  test('a symlink as a frame is read as the file it points at, and refused as not a frame', () => {
    const root = copyRepo();
    const f = path.join(root, 'pets/bellwether/src/frames/walk_01.txt');
    fs.rmSync(f);
    fs.symlinkSync('/etc/hosts', f);
    const r = build(root, '--check');
    assert.equal(r.status, 1);
    assert.match(r.stderr, /^build-pet: /m);
  });
});

describe('overlays: only well-formed, integer, in-range entries come through', () => {
  const lock = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'pets.lock.json'), 'utf8'));
  test('the pinned sleeping z sits at mirrored x for a sheep facing right, with the same size', () => {
    const ov = cleanOverlays(lock.bundled.bellwether.overlays, 64);
    const [frame] = Object.keys(ov);
    const [l] = overlayPlacements(ov, +frame, { x: 100, y: 50, width: 60, tileW: 40, flip: false });
    const [r] = overlayPlacements(ov, +frame, { x: 100, y: 50, width: 60, tileW: 40, flip: true });
    assert.equal(l.dw, r.dw);
    assert.equal(l.dh, r.dh);
    assert.equal(l.dy, r.dy);
    assert.ok(Math.abs(l.dx - 100 + (r.dx - 100) + l.dw - 60) <= 1, 'the two placements are mirror images across the 60 px tile (rounding to a pixel)');
  });
  test('prototype keys, strings, floats, infinities, out-of-range tiles and oversize boxes are dropped; extra fields are not carried', () => {
    const raw = JSON.parse('{"__proto__":[{"tile":0,"x":1,"y":1,"w":1,"h":1}],"constructor":[{"tile":0,"x":1,"y":1,"w":1,"h":1}],"1":"x","2":[null,1,"a",{"tile":"0","x":1,"y":1,"w":1,"h":1},{"tile":0,"x":1.5,"y":1,"w":1,"h":1},{"tile":0,"x":1e400,"y":1,"w":1,"h":1},{"tile":99,"x":0,"y":0,"w":1,"h":1},{"tile":-1,"x":0,"y":0,"w":1,"h":1},{"tile":0,"x":0,"y":0,"w":65,"h":1},{"tile":0,"x":0,"y":0,"w":0,"h":1},{"tile":0,"x":0,"y":0,"w":1,"h":1,"src":"javascript:1"}],"-3":[],"3.5":[]}');
    const out = cleanOverlays(raw, 64);
    assert.deepEqual(out, { 2: [{ tile: 0, x: 0, y: 0, w: 1, h: 1 }] });
    assert.equal(Object.getPrototypeOf(out), Object.prototype);
    assert.equal({}.tile, undefined, 'nothing leaked onto Object.prototype');
    for (const bad of [null, undefined, 5, 's', [], [1], true]) assert.deepEqual(cleanOverlays(bad, 64), {});
  });
});

describe('the bundled sheet itself, decoded independently of the script', () => {
  test('RGBA with no gamma, time or text chunks, no #FF00FF, no partial alpha, and nothing outside the mask tones in the tinter\'s hue window', () => {
    const xml = fs.readFileSync(path.join(AGENT_DIR, 'pets/bellwether/animations.xml'), 'utf8');
    assert.doesNotMatch(xml, /<!(DOCTYPE|ENTITY)|<transparency/i);
    const png = Buffer.from(/<png>([\s\S]*?)<\/png>/.exec(xml)[1], 'base64');
    const chunks = [];
    for (let p = 8; p < png.length; p += 12 + png.readUInt32BE(p)) chunks.push(png.toString('latin1', p + 4, p + 8));
    assert.deepEqual(chunks, ['IHDR', 'IDAT', 'IEND']);
    assert.equal(png[25], 6, 'colour type RGBA');
  });
});

const BIN = process.env.BELLWETHER_BIN;
describe('reserved names, through the real CLI (BELLWETHER_BIN)', { skip: !BIN && 'set BELLWETHER_BIN to a built bellwether-shell' }, () => {
  const run = (args, env) => spawnSync(BIN, args, { encoding: 'utf8', timeout: 60_000, env: { PATH: process.env.PATH, ...env } });
  const mk = () => {
    const base = tempDir('m10a-cli-');
    const home = path.join(base, 'home');
    const bw = path.join(base, 'bw');
    fs.mkdirSync(home);
    fs.mkdirSync(bw);
    assert.ok(!isRealHome(home));
    return { home, bw, env: { HOME: home, BELLWETHER_HOME: bw } };
  };
  const src = path.join(AGENT_DIR, 'pets/bellwether/animations.xml');
  const names = ['bellwether', 'brah', 'BELLWETHER', 'Brah', 'bEllWeThEr', 'bellwether ', 'ｂｅｌｌｗｅｔｈｅｒ', 'belĺwether', 'brah​', 'bellwether/../brah', '..'];
  for (const name of names) {
    test(`--name ${JSON.stringify(name)} is refused and nothing is written`, () => {
      const { bw, env } = mk();
      const r = run(['pets', 'import', src, '--name', name], env);
      assert.notEqual(r.status, 0, r.stdout);
      assert.deepEqual(fs.readdirSync(bw), [], 'nothing written');
    });
  }
  test('a pet in a folder named brah, with no --name, is refused', () => {
    const { bw, env } = mk();
    const d = path.join(tempDir('m10a-brah-'), 'brah');
    fs.mkdirSync(d);
    fs.copyFileSync(src, path.join(d, 'animations.xml'));
    const r = run(['pets', 'import', path.join(d, 'animations.xml')], env);
    assert.notEqual(r.status, 0);
    assert.deepEqual(fs.readdirSync(bw), []);
  });
  // Was a finding (M10a review D4): "bellwether." (trailing dot) read as the sheep and was
  // accepted. Trailing dots and spaces are now refused as the bundled name.
  test('a trailing dot is also refused', () => {
    const { bw, env } = mk();
    const r = run(['pets', 'import', src, '--name', 'bellwether.'], env);
    assert.notEqual(r.status, 0);
    assert.deepEqual(fs.readdirSync(bw), []);
  });
  // Was a defect (M10a review D1): doctor said PASS for a name the strip refuses.
  for (const name of ['brah', 'BELLWETHER']) {
    test(`doctor does not PASS pets.default ${name}, which the strip refuses to load`, () => {
      const { bw, env } = mk();
      fs.writeFileSync(path.join(bw, 'config.json'), JSON.stringify({ port: 47812, pets: { default: name } }));
      // Named explicitly: the suite's isolation preload otherwise hands the child its own scratch BELLWETHER_CONFIG.
      const r = run(['doctor'], { ...env, BELLWETHER_CONFIG: path.join(bw, 'config.json') });
      const line = r.stdout.split('\n').find((l) => /\bpets\b/.test(l)) ?? '';
      assert.doesNotMatch(line, /PASS/, line);
    });
  }
});

describe('build-pet is not a silent no-op', () => {
  // Was a finding (M10a review D3): main() was skipped when argv[1] was a symlinked path.
  test('run through a symlinked path, --check still says something (a gate that prints nothing and exits 0 is no gate)', () => {
    const root = copyRepo();
    const link = path.join(tempDir('m10a-link-'), 'checkout');
    fs.symlinkSync(root, link);
    fs.appendFileSync(path.join(root, 'pets/bellwether/animations.xml'), '\n');
    const r = spawnSync(process.execPath, [path.join(link, 'scripts', 'build-pet.mjs'), '--check'], { encoding: 'utf8' });
    assert.equal(r.status, 1, `exit ${r.status}, stdout ${JSON.stringify(r.stdout)}`);
  });
});
