# Agent briefs

Fill in the `<placeholders>` and send each brief as the complete prompt to a fresh sub-agent.
Each agent starts with none of your context, so the brief has to stand alone: give it the issue
numbers, branch, worktree path and the epic's purpose in one or two sentences of your own.

Every brief ends with the same rule: when the agent hits something from the "When to stop and
ask" list in SKILL.md, it stops and reports the question instead of guessing.

Add an environment paragraph to every brief with what SKILL.md's environment checks found: how
to read GitHub (REST if GraphQL is blocked), how to open the PR, whether Docker runs, how to run
crawler dev verification (manual mode, swapped demo shops), host-specific test failures to
ignore, and the commit trailers the session requires. Tell agents not to put model identifiers
anywhere but the trailers. Tell them to stop every background process they start (dev
binaries, watch loops) before reporting; a wait loop on `pgrep -f` matches its own command line
and never ends.

## Implementer

```text
You are implementing GitHub issue #<child> in aura-historia/backend as part of epic #<epic>
("<epic title>": <one-sentence purpose>). Work only in the worktree <worktree path>, on branch
<branch>, which starts at the tip of the epic branch <epic-branch>.

1. Read the issue (`gh issue view <child> --comments`), the epic (`gh issue view <epic>`) and the
   PRs already merged into the epic branch that this issue builds on: <merged sibling PRs or "none">.
2. Follow AGENTS.md and any nested AGENTS.md. Use the repository skills that apply, in particular
   refactor-while-implementing, and crawler-dev-verify for crawler runtime changes.
3. Implement the issue completely within its scope. Do not widen it. Note anything outside it as
   a follow-up finding with `path:line`. Ask whether the change alters the raw values extracted
   from unchanged HTML and therefore needs an `EXTRACTOR_VERSION` raise under the rule in
   docs/crawler/extraction-and-review.md (extraction contract), and say in the report which way
   you decided.
4. Run the focused validation for the crates you changed (fmt, check, clippy with -D warnings,
   tests, all with --locked) and fix what fails.
5. Commit with subjects `<type>(#<child>): <summary>`, keeping structural and behavioural changes
   in separate commits. Do not push and do not open a PR.

Stop and report instead of guessing when the issue is ambiguous, contradicts the code or the
epic, or needs a public-contract, persisted-format or cross-crate change it does not mention.

Report in at most 25 lines:
- STATUS: done | blocked
- COMMITS: <sha> <subject>, one per line
- CHANGES: what you changed and why, in 3-6 bullets
- VALIDATION: each command and its result
- CONTRACTS: swagger/CHANGELOG/runbook/migration changes, or "none"
- FOLLOW-UPS: findings outside scope, with path:line
- QUESTIONS: what needs the user, or "none"
```

## Reviewer

```text
You are the independent reviewer for GitHub issue #<child> in aura-historia/backend, part of epic
#<epic> ("<epic title>"). Another agent implemented it; you did not see that work and owe it no
deference. Work in <worktree path> on branch <branch>. The change under review is
`git diff origin/<epic-branch>...HEAD`.

Implementer's report:
<paste the implementer's report>

1. Read the issue, the epic, AGENTS.md and the canonical docs the change touches. Review the diff
   for correctness, completeness against the issue's acceptance criteria, architecture rules,
   error handling, test strength and contract or documentation drift. Check whether the change
   alters the raw values extracted from unchanged HTML and therefore needs an `EXTRACTOR_VERSION`
   raise under the rule in docs/crawler/extraction-and-review.md (extraction contract), whatever
   the implementer decided. Use the applicable repo review skills as lenses (aura-bug-hunt,
   aura-test-quality, aura-contracts, aura-data-events, aura-security) without turning this into
   a whole-repository audit.
2. Apply the fixes you judge sensible as your own commits `<type>(#<child>): <summary>`. Leave
   matters of taste alone. Do not rewrite the approach unless it is wrong.
3. Run the focused validation again (--locked) and, for crawler runtime changes, crawler-dev-verify.
4. Push the branch (`git push -u origin "<branch>"`) and open a PR into the epic branch:
   gh pr create --base "<epic-branch>" --head "<branch>" --title "<type>(#<child>): <summary>"
   Body: "Part of #<epic>; implements #<child> against the epic integration branch." Then a
   summary, an "Independent review and validation" section saying an independent review agent reviewed it, what review
   changed and the validation results, and follow-up findings. Do not write "Closes #<child>":
   GitHub only closes issues from the default branch.
5. If the orchestrator watches CI (PR subscriptions, or `gh pr checks` unavailable), report as
   soon as the PR is open. Otherwise wait for CI with one blocking `gh pr checks <pr> --watch`
   run in the background, fix failures your change caused, push and wait again; for a failure
   unrelated to the change, rerun the failed jobs once (`gh run rerun <run-id> --failed`), and
   if it fails again, stop and report it. Do not merge. The orchestrator merges.

Never skip tests, disable checks, use --no-verify or force-push. Stop and report instead of
guessing on design disagreements the docs do not settle.

Report in at most 25 lines:
- STATUS: green | red | blocked
- PR: <number and url>, head <sha>
- REVIEW FIXES: what you changed and why, in bullets, or "none"
- VALIDATION: each command and its result
- CI: final state per failing or rerun job, with links
- FOLLOW-UPS and QUESTIONS, as for the implementer
```

## Conflict or CI fixer

```text
PR #<pr> (branch <branch>, issue #<child>, epic #<epic>) cannot merge into <epic-branch>:
<"it is behind/conflicting after #<sibling PRs> merged" | "CI fails: <summary and links>" |
"a review bot reports findings this PR introduced: <rule, path:line, message>">.
Work in <worktree path>.

1. `git fetch origin` and merge origin/<epic-branch> into the branch (no rebase, no force-push).
2. Resolve conflicts so both sides' behaviour is kept. If a resolution would change what either
   side does, stop and report the conflict instead of choosing.
3. Revalidate the affected crates (--locked), push, and wait for CI with one blocking
   `gh pr checks <pr> --watch` run in the background. Handle unrelated failures as in the reviewer
   brief: one rerun of failed jobs, then report.

Report in at most 15 lines: STATUS (green | red | blocked), head sha, what you resolved or fixed,
validation, CI state, questions.
```

## Final epic reviewer

```text
You are the final reviewer of epic #<epic> ("<epic title>") in aura-historia/backend. All child
issues are integrated into <epic-branch>: <child -> PR list>. Work in the primary checkout
<checkout path>, which is on <epic-branch> itself, so you push fixes directly to it. Leave its
untracked files alone.

1. `git fetch origin` and merge origin/develop into the epic branch (merge, no rebase). Resolve
   conflicts, keeping both sides' behaviour. Stop and report a conflict that would change either.
2. Review `git diff origin/develop...HEAD` as one change against the epic and all child issues.
   Look for what per-child reviews could not see: inconsistencies between children, duplicated
   or dead code across them, missing integration, broken architecture boundaries, contract and
   documentation drift (docs/swagger.yaml, docs/CHANGELOG.md, owning docs and runbooks),
   migration ordering, and gaps against the epic's acceptance criteria. Use the aura review
   skills as lenses (aura-architecture, aura-bug-hunt, aura-contracts, aura-data-events,
   aura-test-quality, aura-security).
3. Fix what is sensible as commits `<type>(#<epic>): <summary>`. Anything that should not be fixed
   in this epic is a follow-up finding.
4. Run the broad validation from AGENTS.md (fmt, check, clippy, depgraph-check, workspace lib
   tests, infra tests and synth, all with --locked where applicable). Fix what fails.
5. Push to <epic-branch>. Do not open the PR to develop and do not merge anything.

Report in at most 30 lines:
- STATUS: done | blocked
- HEAD: <sha>
- DEVELOP MERGE: clean | conflicts resolved (which files, how)
- REVIEW FIXES: bullets
- VALIDATION: each command and its result
- CONTRACTS: public, persisted or operational changes the epic makes
- FOLLOW-UPS and QUESTIONS
```
