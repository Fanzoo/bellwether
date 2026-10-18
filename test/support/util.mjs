/**
 * A test driver, copied from the Node brain at v0.2.0-rc.3 (brain/util.mjs): the CLI's
 * exit codes and the error the drivers here throw. Not the brain: no test checks it as one.
 */

/** Exit codes: 0 ok, 1 unexpected, 2 not running / not installed, 3 bad config. */
export const EXIT = { OK: 0, UNEXPECTED: 1, NOT_RUNNING: 2, BAD_CONFIG: 3 };

/** A failure with a specific exit code (what test/impl.mjs throws for a Rust `{error}`). */
export class CliError extends Error {
  constructor(message, exitCode = EXIT.UNEXPECTED) {
    super(message);
    this.exitCode = exitCode;
  }
}
