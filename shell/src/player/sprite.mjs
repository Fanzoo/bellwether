import { frameAt, motionAt, pickNext, totalSteps } from './stepper.mjs';

/**
 * One sheep on the strip: which behaviour it shows, which animation and frame it is
 * on, where it stands, and which way it faces. No DOM: `tick(now)` advances it and
 * returns true when what should be drawn changed.
 *
 * The brain decides the behaviour (busy, idle, sleeping, …); the pet file only
 * supplies the animations. A walking sheep wanders within `wander` px of its home
 * slot and never leaves the strip: at either bound it turns round.
 */

/** Snapshot actor → behaviour name (the keys of config `pets.animationMap`). */
export function behaviourOf(actor) {
  if (actor.state === 'leaving') return 'leave';
  if (actor.alert) return 'alert';
  switch (actor.state) {
    case 'arriving':
      return 'arrive';
    case 'busy':
      return actor.flavour === 'running' ? 'run' : 'walk';
    case 'idle':
      return 'graze';
    case 'sleeping':
      return 'sleep';
    case 'done':
      return 'sit';
    case 'waiting':
      return 'lie';
    case 'compacting':
      return 'chew';
    case 'error':
      return 'dizzy';
    default:
      return 'graze';
  }
}

const MOVING = new Set(['walk', 'run', 'arrive', 'leave']);
// Behaviours that slide back to their range rather than walk there: walking would wake
// a sleeping or lying sprite, or turn an alerting one away from you.
const SLIDES = new Set(['sleep', 'lie', 'dizzy', 'alert']);
// How far outside its range a resting sprite may be before it walks back.
const STRAY_PX = 3;

export class Sprite {
  /**
   * @param {object} o
   * @param {object} o.model      PetModel
   * @param {object} o.map        behaviour -> [animation ids] (resolveAnimationMap)
   * @param {number} o.width      tile width in px (after scaling)
   * @param {number} o.height     tile height in px
   * @param {number} o.stripWidth
   * @param {number} o.home       x of the home position (left edge)
   * @param {number} o.wander     how far it may stray from home
   * @param {() => number} [o.rand]
   * @param {(msg: string) => void} [o.warn]
   */
  constructor({ model, map, width, height, stripWidth, stripHeight, home, wander, rand = Math.random, warn = () => {} }) {
    Object.assign(this, { model, map, width, height, stripWidth, stripHeight, home, wander, rand, warn });
    this.randS = Math.floor(rand() * 100);
    this.x = this.clampToStrip(home);
    this.facing = rand() < 0.5 ? -1 : 1; // the pet art faces left (-1)
    this.behaviour = null;
    this.oneShot = null;
    this.gone = false;
    this.anim = null;
    this.nextDue = 0;
    this.frame = 0;
    this.offsety = 0;
    this.opacity = 1;
    this.holding = false; // alert: turned to face forward, holding that frame
    this.confirmed = false;
    this.alertSince = 0;
    this.returnTo = null; // lambs: () => the x to trot back to when leaving (the parent)
    this.catchingUp = false; // walking back to its range, then resuming its behaviour
  }

  env() {
    return {
      screenW: this.stripWidth,
      screenH: this.stripHeight,
      areaW: this.stripWidth,
      areaH: this.stripHeight,
      imageW: this.width,
      imageH: this.height,
      imageX: this.x,
      imageY: this.stripHeight - this.height,
      random: this.rand() * 100,
      randS: this.randS,
    };
  }

  /** Switches behaviour; the first animation of the new behaviour starts at once. */
  setBehaviour(name, now) {
    if (name === this.behaviour && !this.oneShot) return;
    const wasAlert = this.behaviour === 'alert';
    this.catchingUp = false;
    this.behaviour = name;
    this.holding = false;
    if (name === 'alert' && !wasAlert) {
      // Stop and turn to face the user: the pet's turn animation, then hold its last frame
      // (front-facing for esheep64) while the shell bounces the sprite.
      this.alertSince = now;
      this.confirmed = false;
      this.oneShot = null;
      this.start(this.idsFor('turn')[0], now);
      return;
    }
    if (name === 'leave') this.facing = this.x + this.width / 2 < this.stripWidth / 2 ? -1 : 1;
    if (this.oneShot) return; // the hop finishes first, then this behaviour starts
    this.start(this.idsFor(name)[0], now);
  }

  setHome(home) {
    this.home = home;
  }

  /**
   * The ~6 s Notification confirmed the alert: escalate visually, once. The pet's
   * alert animation (esheep64's boing) plays, then the sheep faces forward again,
   * bouncing higher. No sound (Decision 7).
   */
  setConfirmed(confirmed, now) {
    if (confirmed && !this.confirmed && this.behaviour === 'alert') {
      this.holding = false;
      this.playOnce('alert', now);
    }
    this.confirmed = !!confirmed;
  }

  /** True while the sprite moves every frame regardless of its animation (the bounce). */
  get bouncing() {
    return this.behaviour === 'alert';
  }

  /** A one-off animation (the hop when clicked), then back to the behaviour. */
  playOnce(name, now) {
    const ids = this.map[name];
    if (!ids) return;
    this.holding = false;
    this.oneShot = name;
    this.start(ids[0], now);
  }

  idsFor(name) {
    return this.map[name] ?? this.map.walk ?? [Number(Object.keys(this.model.animations)[0])];
  }

  start(id, now) {
    const anim = this.model.animations[id];
    const env = this.env();
    this.anim = { id, step: 0, total: Math.max(1, totalSteps(anim, env, this.warn)) };
    this.nextDue = now;
  }

  /** The x range it may wander in: home ± wander, within the strip. */
  range() {
    return [Math.max(0, this.home - this.wander), Math.min(this.stripWidth - this.width, this.home + this.wander)];
  }

  /** How far outside its range it is (0 inside; negative: to the left). */
  stray() {
    const [lo, hi] = this.range();
    return this.x < lo ? this.x - lo : this.x > hi ? this.x - hi : 0;
  }

  /** Advances to `now`. Returns true if the drawing changed. */
  tick(now) {
    if (this.gone || !this.anim) return false;
    const slid = this.slideBack();
    if (this.holding || now < this.nextDue) return slid;
    const current = this.oneShot ?? this.behaviour;
    // A resting sprite whose home moved away (a lamb whose parent walked on, a sheep
    // whose family band shifted) walks back, then carries on resting.
    if (!this.catchingUp && !this.oneShot && !MOVING.has(current) && !SLIDES.has(current) && Math.abs(this.stray()) > STRAY_PX) {
      this.catchingUp = true;
      this.start(this.idsFor('walk')[0], now);
    }
    const anim = this.model.animations[this.anim.id];
    const env = this.env();
    const m = motionAt(anim, this.anim.step, this.anim.total, env, this.warn);
    this.frame = frameAt(anim, this.anim.step);
    this.offsety = m.offsety;
    this.opacity = m.opacity;
    if (MOVING.has(current) || this.catchingUp) this.move(Math.abs(m.dx), this.catchingUp ? 'walk' : current);
    if (this.catchingUp && this.stray() === 0) {
      this.catchingUp = false;
      this.start(this.idsFor(this.behaviour)[0], now);
      return true;
    }
    this.nextDue = now + m.interval;
    this.anim.step++;
    if (this.anim.step >= this.anim.total) this.finished(anim, now);
    return true;
  }

  move(speed, behaviour) {
    const step = Math.max(1, speed);
    if (behaviour === 'leave') {
      const target = this.returnTo?.();
      if (target != null) {
        // A lamb trots back into its parent and is gone when it gets there.
        const d = target - this.x;
        this.facing = Math.sign(d) || this.facing;
        const pace = Math.max(step * 2, Math.abs(d) / 6);
        if (Math.abs(d) <= pace) {
          this.x = target;
          this.gone = true;
        } else this.x += this.facing * pace;
        return;
      }
      // Run off the nearer edge; the shell drops the sprite once it is gone.
      this.x += this.facing * step * 2;
      if (this.x < -this.width || this.x > this.stripWidth) this.gone = true;
      return;
    }
    const lo = Math.max(0, this.home - this.wander);
    const hi = Math.min(this.stripWidth - this.width, this.home + this.wander);
    if (this.x < lo || this.x > hi) {
      // Outside its range (the home moved: a lamb trotting out, or whose parent walked
      // on): head there, quickly while far (an eighth of the way each step), rather
      // than jumping there.
      const d = this.x < lo ? lo - this.x : hi - this.x;
      this.facing = Math.sign(d);
      const pace = Math.abs(d) > 30 ? Math.max(step * 2, Math.abs(d) / 8) : step;
      this.x += Math.sign(d) * Math.min(Math.abs(d), pace);
      return;
    }
    let x = this.x + this.facing * step;
    if (x <= lo || x >= hi) {
      x = Math.min(Math.max(x, lo), hi);
      this.facing = -this.facing;
    }
    this.x = x;
  }

  finished(anim, now) {
    if (this.catchingUp) {
      this.start(this.idsFor('walk')[0], now);
      return;
    }
    if (this.behaviour === 'alert') {
      // After the turn (or the escalation), face forward and hold.
      if (this.oneShot) {
        this.oneShot = null;
        this.start(this.idsFor('turn')[0], now);
        return;
      }
      this.holding = true;
      return;
    }
    if (this.oneShot) {
      this.oneShot = null;
      this.start(this.idsFor(this.behaviour)[0], now);
      return;
    }
    // Stay within the behaviour: follow the pet's own `next` when it points at one of
    // this behaviour's animations (sleep1a -> sleep1b); otherwise move on to the
    // behaviour's next animation, and loop the last one.
    const ids = this.idsFor(this.behaviour);
    const next = pickNext(anim, this.rand(), new Set(ids));
    const i = ids.indexOf(anim.id);
    const fallback = i === -1 ? ids[0] : ids[Math.min(i + 1, ids.length - 1)];
    this.start(next ?? fallback, now);
  }

  /** Sleeping, lying, dizzy or alerting and out of range: slide back, an eighth of the way a frame. */
  slideBack() {
    if (!SLIDES.has(this.oneShot ?? this.behaviour)) return false;
    const d = -this.stray();
    if (Math.abs(d) <= STRAY_PX) return false;
    this.x += Math.sign(d) * Math.min(Math.abs(d), Math.max(1.5, Math.abs(d) / 8));
    return true;
  }

  clampToStrip(x) {
    return Math.min(Math.max(0, x), Math.max(0, this.stripWidth - this.width));
  }

  /** Pixels the sprite is lifted by the alert bounce at `now` (0 when calm). */
  bounce(now) {
    if (this.behaviour !== 'alert' || !this.holding) return 0;
    const period = this.confirmed ? 420 : 640;
    const amplitude = this.confirmed ? 9 : 4;
    return Math.round(amplitude * Math.abs(Math.sin(((now - this.alertSince) / period) * Math.PI)));
  }

  /** What to draw: the frame, where, and whether to mirror it (the art faces left). */
  drawInfo(now) {
    const lift = now === undefined ? 0 : this.bounce(now);
    return {
      frame: this.frame,
      x: Math.round(this.x),
      y: Math.round(this.stripHeight - this.height - this.offsety - lift),
      flip: this.facing > 0 && !this.holding,
      opacity: this.opacity,
    };
  }
}
