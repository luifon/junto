# Step 0 spikes

Each directory here is one technical test from step 0 of the design. A spike
answers a stated question with a pass or fail result and the evidence for it.
A spike is not product code and is not reused as product code.

## Rules for every spike

- Work only inside your own spike directory. Do not run `git` commands that
  change the repository (no add, commit, checkout, stash, reset).
- Write the result to `RESULT.md` in the spike directory: the question, the
  method, the exact commands, the observed output, and PASS or FAIL for each
  stated gate. Mark each statement as observed (you ran it and saw it) or not
  verified. Do not report a gate as passed without observing it.
- On macOS, run every compile or other CPU-heavy command through
  `tools/serial.sh` from the repository root, for example
  `../../tools/serial.sh cargo build --release`. It makes heavy work run one
  at a time. Do not start a second heavy command in parallel.
- No personal or operator-specific values in any file: no person names, user
  names, home-directory paths, host names, domain names, IP addresses, keys or
  tokens. Name a machine by its role in the test ("the data host", "the public
  host", "the Linux test machine"). A script reads the SSH name of a machine
  from an environment variable that has no default, for example
  `JUNTO_TEST_DATA_HOST`. Values that must be stored go in `.env.local`, which
  is not tracked.
- Keep comments in code to a minimum. Code states what it does through names.
- Dependencies must have a licence that Apache-2.0 software can use. Do not
  copy code from GPL projects, and do not read the GPL-licensed crates of the
  Zed editor. The Apache-2.0 crates `gpui*` are allowed.
- When you finish, delete build output you no longer need (`target/`,
  downloaded archives, containers and images you created) and say in
  `RESULT.md` what you left in place and why.
