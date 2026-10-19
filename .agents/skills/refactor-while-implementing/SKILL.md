---
name: refactor-while-implementing
description: "Use whenever implementing a GitHub issue, ticket, feature, bug fix or any code change in aura-historia/backend, from the first read of the code to the final commit. It keeps the code the change touches modular, readable and maintainable by tidying before the change, shaping the change itself, and finishing with a structure pass, all without widening the ticket. Apply it even when the user only says 'implement #NNNN', 'fix this' or 'add X', and before any edit that spans more than one function."
---

# Refactor while implementing

Every ticket leaves the code it touches either a little clearer or a little more tangled. The
tangle is never one ticket's fault: it is the sum of changes added where they landed. This
repository already carries the result, such as a 3,600-line cron scheduler file in the crawler.
This skill makes the small structural work part of implementing, so each ticket pays its share
instead of adding to the debt, and it bounds that work so the ticket still ships.

Two ideas drive it. **Tidy first**: when the code is awkward to change, make the change easy
with small behaviour-preserving tidyings committed on their own, then make the easy change.
**Leave it better than you found it, within reach**: improve only the code the change passes
through. Everything else becomes a note for a ticket, never a detour.

## Scope rule

- Stay in the crates the ticket owns. A crawler ticket changes `src/crawler` (and
  `docs/crawler`) only. When the change seems to need behaviour from another crate, stop,
  describe the needed change with `path:line` in the summary, and leave it to the user.
- Touch only code the change reads or modifies: the functions on its path, their direct callers
  and their tests. A smell two modules away is a finding for a ticket, not a tidying.
- Keep structural and behavioural changes in separate commits. A reviewer must be able to read
  the tidying commits as "no behaviour change, tests unchanged" and the feature commit as "this is
  the change", each on its own.

## Workflow

1. **Map before touching.** Read the ticket and the owning document (`docs/arch.md` for layering,
   the specialised doc or runbook for the contract, nested AGENTS.md). List the functions the
   change passes through and read them whole, with their callers and tests. Write down, in one
   line each, what makes the change hard: a duplicated branch, a function doing three jobs, a
   mapping in the wrong layer, a `foo`/`foo_with_x` sibling pair, a boolean parameter steering
   behaviour. That list is the tidying plan; keep it short.
2. **Tidy first.** Apply the tidyings from the plan as small, behaviour-preserving steps, run the
   focused tests after each, and commit them together as `refactor(#NNNN): <what>` before any
   behaviour changes. Time-box it: a tidying that is not green within about twenty minutes, or
   that keeps pulling in code outside the path, is a ticket. Revert it and move on.
3. **Make the change.** Build the behaviour in the shape the tidying prepared, following the
   design rules below. Put new behaviour where the architecture says it lives, not where the
   call happens to be. Write or extend the tests next to the behaviour they prove.
4. **Finishing pass.** Before committing, re-read the whole diff as the reviewer will. Ask: can
   each function be named by one verb phrase; does each file still have one reason to change;
   did the change add a variant, a flag or a copy where a signature change would do; does every
   error keep its cause; could a newcomer follow the path from caller to effect without the
   ticket? Fix what the answers expose, as a second structural commit when it is separable.
5. **Validate and hand over.** Run the checks from AGENTS.md (`fmt`, `check`, `clippy` with
   `-D warnings`, the crate's tests, `--locked`). For crawler changes also run the
   `crawler-dev-verify` skill. The PR summary states what was tidied and why, and lists the
   findings left for tickets with `path:line`.

## Design rules for the new code

| Concern | Rule |
|---|---|
| Placement | Domain behaviour in the core crate, use-case orchestration and transaction scope in the service crate, storage and provider mapping inside the adapter that owns the representation, thin runtime and transport layers (`docs/arch.md`, core rules). |
| Functions | One job per function, named by that job. Extract when a function needs a comment to separate its phases, exceeds roughly sixty lines, or nests more than three levels. |
| Modules | A file approaching eight hundred lines, or gaining a second responsibility, is split by responsibility, the way `scraper_service/pipeline/` splits cached selection, fresh generation and application. |
| Variants | Change the signature and every caller instead of adding `foo_up_to`, `foo_with_x` or a wrapper sibling. A boolean that selects behaviour becomes two functions or an enum. |
| Duplication | A match or branch that appears twice becomes one owner only when both sites share validation, failure behaviour and meaning. Lookalikes with different bounded-context meaning stay apart. |
| Types | Route input through existing newtype constructors so validation cannot be bypassed. When the change introduces a state, make the invalid combinations unrepresentable with an enum or newtype rather than a comment. |
| Errors | Preserve causes with `#[source]` and `application::error::BoxError`. Fail closed on invalid persisted state and untrusted input; never turn a failure into a default to shorten a branch. |
| Contracts | Persisted formats, public identifiers, event and API shapes stay stable. A change to any of them is its own ticket with docs, `docs/swagger.yaml` and `docs/CHANGELOG.md` updates. |
| Comments | A comment that explains what the code does is a naming problem; rename or extract. Keep comments for why: an invariant, a deliberate deviation, a link to the owning doc. |
| Tests | One scenario per test module, named by the behaviour (`should_<behaviour>_when_<condition>`), assertions on the outcome the ticket cares about, no tautologies. Extend the fixture-driven suites where they exist. |
| Docs | Amend the owning document once; `docs/arch.md` and `docs/storage.md` stay design guides, never inventories. |

## Permitted tidyings

Rename for clarity. Extract a function or a module along a responsibility seam. Inline a
one-caller indirection that hides the flow. Replace a sibling-variant pair with one signature.
Group parameters that always travel together into a struct. Replace nested conditionals with
guard clauses. Move a mapping to the boundary that owns the representation. Delete private code
the change makes unreachable, after searching every reference including tests, feature gates,
string registration and generated code.

## Not tidyings

These need their own ticket and review, however tempting while in the file: public API, DTO,
event or persisted-format changes; renames of persisted identifiers or enum values; adding,
removing or updating dependencies; moving code across crates; repository-wide reformatting or
lint allowances; rewriting tests to match new behaviour; weakening error semantics; generic
utility hubs; an abstraction with one caller; any "while I am here" change outside the path.

## Commits

Structural commits first, `refactor(#NNNN): <what>`, each message stating that behaviour and
tests are unchanged. Then the behavioural commit, `fix(#NNNN): …` or `feat(#NNNN): …` in the
repository's style. A separable finishing tidy is a second `refactor` commit. Never fold a
tidying into the behavioural commit to save a step: the split is what makes both reviewable.

## Done means

- The ticket's behaviour is implemented, tested and validated with the AGENTS.md checks.
- The code on the change's path reads better than before, and nothing outside it moved.
- Structural and behavioural commits are separate and each message says which it is.
- The summary lists what was tidied, what was deliberately left, and the ticket-worthy
  findings with `path:line`.
