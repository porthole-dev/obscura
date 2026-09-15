# AI disclosure and responsibility

## How this project is made

This project is developed with substantial help from AI coding assistants,
mainly Anthropic's Claude. Most code, patches, scripts and documentation were
drafted by the model under the maintainer's direction. The maintainer decides
what is built, reviews it, and tests it on real devices.

## How it is marked

- Every commit by the maintainer carries an `Assisted-by: Claude` trailer
  (`Assisted-by: LLM` for Linux kernel patches, the form the kernel documents).
  History was rewritten on 2026-09-15 to add it to every earlier commit, because
  the whole project postdates the first AI-assisted session.
- An AI is never listed as `Co-authored-by` and never adds `Signed-off-by`.
  A sign-off is a personal Developer Certificate of Origin, which only a human
  can give.
- Commits by other contributors are unchanged and carry only what they wrote.

## Responsibility

- **Using an AI does not move responsibility anywhere.** Whoever signs off or
  merges a change is responsible for it, exactly as if they had typed it. This
  is the position of the Linux kernel, Mesa and the Linux Foundation, and it is
  this project's.
- **No warranty.** Everything here is provided "as is", without warranty of any
  kind, under the terms of the LICENSE. Flashing or installing it can brick a
  device, erase data, or damage hardware (for example by overheating). You use
  it at your own risk, and the authors are not liable for the result.
- **Unofficial.** Not affiliated with or endorsed by postmarketOS, Alpine Linux,
  Google, Qualcomm or Anthropic. Report problems here, not to them.
- **Licensing.** The output of the tools used is not restricted by their terms.
  Third-party code keeps its original authors and licence. If you believe
  something here reproduces your copyrighted work, open an issue and it will be
  removed.

## Upstream

Patches go to an upstream project only if that project accepts AI-assisted
contributions, only under that project's own rules, and only when a human
sends them. No autonomous tool opens merge requests, issues or comments
anywhere. Nothing from this project is submitted to postmarketOS, whose policy
does not accept AI-generated contributions.
