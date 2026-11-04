---
name: crawler-orchestrate-epic
description: "Own a crawler epic (src/crawler: spider, scraper, schema review) in aura-historia/backend end to end as an orchestrator: plan the next epic from the open crawler tickets when asked to start the next one, read its child issues and their 'blocked by' relationships, implement each child in its own independent session, have a second fresh session review and fix it, open a PR into the epic branch and merge it once CI is green, run all independent children in parallel, then review the whole epic and open (never merge) the PR into develop. Invoke explicitly, e.g. '/crawler-orchestrate-epic 2150', or '/crawler-orchestrate-epic next' to plan and start the next epic."
disable-model-invocation: true
argument-hint: "[epic issue number | next] [model overrides]"
---

# Orchestrate an epic

You are the owner of one epic. Your job is to plan, delegate, verify and merge; sub-agents write
and review the code. Your own context has to last for the whole epic, which can be a dozen child
issues and many hours of CI, so you never read diffs or run builds yourself. You read short agent
reports and `gh` state, and you keep a ledger on disk so a compacted context can resume.

## Defaults the invocation may override

| Role | Where it runs | Default model and effort |
|---|---|---|
| Child implementer | Its own independent session | Sonnet 5.5 (`claude-sonnet-5-5`) |
| Child reviewer | Its own independent session, never the implementer's | Opus 5.5 (`claude-opus-5-5`) |
| Merge-conflict, CI or bot-finding fixer | The child's reviewer session, or a fresh session | Opus 5.5 |
| Final epic reviewer | A fresh session, or a sub-agent of the orchestrator | Opus 5.5, XHigh |

Use exactly the models the user names instead. If the harness cannot select one of them, stop and
ask before substituting. The user chose them on purpose, and a silent fallback would invalidate
the review claims in every PR. Session creation takes a model id but no effort level; a sub-agent
started with the Agent tool takes `model` and `effort` (`opus`, `xhigh`).

Every implementer and every reviewer runs in its own independent session, with its own container,
checkout, Rust target directory and Docker (see [Independent sessions](#independent-sessions)).
Compute on the orchestrator's machine is therefore never the bottleneck, and the number of
children that run at once is limited only by file ownership (see
[step 0](#0-plan-a-new-epic)). Use local sub-agents in worktrees only when the harness cannot
create sessions, and then follow the [fallback](#fallback-local-worktrees).

Every sub-agent is fresh: it starts with no context from this conversation and none from another
agent. A reviewer is never the agent that wrote the code. Independent review is the point of the
second agent, and a continued implementer would just re-approve its own work.

## 0. Plan a new epic

Skip this step when the invocation names an existing epic. When it asks for the next epic ("next",
"start the next wave"):

1. Collect the candidates: the tickets the last crawler epic left open or filed as follow-ups (its
   issue body and its final status comment), open `bug`-labelled crawler issues, and older open
   crawler tickets that are not `parked`. Read each candidate's body.
2. Choose by production impact first: data-corrupting or customer-visible bugs (wrong prices,
   listings that stay live after removal, wrong availability), politeness towards shops,
   fail-closed handling of persisted or untrusted state, and what an operator needs to see once
   many sources run. Refactors, UX polish and coverage work come after them unless the user asks
   for them.
3. Size the epic by file ownership, not by compute. Take as many tickets as can run in parallel
   with one owner per function in shared files, about nine or ten. Defer a ticket that would
   rewrite code across most other tickets (for example log lines everywhere) to its own epic, and
   defer a ticket that needs the same function as a chosen one.
4. Ask the user once, in one dialog: the proposed scope with what is left out and why, and every
   product-owner decision a chosen ticket still needs, each with a recommended option. Do not
   start children whose decisions are open.
5. Create the epic issue (issue type `Epic`, labels `crawler` plus `spider` or `scraper` as they
   apply). The title is neutral, `[Epic] Crawler: <what it delivers>`, with no wave, round or
   priority words. The body follows the earlier crawler epics: Goal (with the `develop` commit the
   tickets were checked against), the epic branch, a Tickets table (child branch, ticket, summary,
   owning epic), what was left out on purpose, the product-owner decisions with their date, Done
   when, and Scope and order with the per-function file ownership. Tickets stay sub-issues of their
   owning epics; attach a ticket to the new epic only if it has no parent.
6. Create the epic branch `epic/#<N>-<slug>` from `origin/develop` with a short, neutral slug, and
   push it before the first session starts: `git push origin "origin/develop:refs/heads/epic/#<N>-<slug>"`.
   A session whose source branch is missing on `origin` fails at start with `ref_not_found`. The
   session's own designated branch is not the epic branch; leave it for changes outside the epic.

## 1. Establish the epic

1. Resolve the epic number from the arguments, or from the checked-out branch
   (`epic/#<N>-<slug>`). Confirm the branch exists on `origin` and the local copy matches it.
   If it is missing, unpushed or diverged, ask.
2. Read the epic issue and load the child graph in one query (sub-issues and their blockers).
   Where GraphQL is blocked (Claude Code cloud sessions), use REST: `gh api
   repos/aura-historia/backend/issues/<N>/sub_issues` and, per child,
   `.../issues/<child>/dependencies/blocked_by`; if that returns nothing, take the blockers from
   the issue bodies and say so in the plan:

   ```sh
   gh api graphql -f query='query{repository(owner:"aura-historia",name:"backend"){issue(number:<N>){title body state subIssues(first:100){nodes{number title state stateReason labels(first:20){nodes{name}} blockedBy(first:50){nodes{number state repository{nameWithOwner}}}}}}}}'
   ```

3. Classify each child:
   - **Done:** already closed (including "not planned"). It counts as satisfied for dependents.
   - **Blocked:** has an open blocker. A blocker inside the epic is satisfied once its PR has been
     merged into the epic branch. A blocker outside the epic is satisfied only when it is closed.
     If an external blocker is still open, ask.
   - **Ready:** every blocker is satisfied.
4. Stop and ask if the graph has a cycle, a child has no actionable acceptance criteria, or the
   epic text contradicts a child.
5. Write the ledger (see below) and report the plan to the user in a few lines: the chains, what
   runs in parallel first, and anything skipped. Then start without waiting, unless something
   above needed an answer.

### Ledger

Keep `epic-<N>-ledger.md` in a git-ignored location or in the session scratchpad
(`.claude/worktrees/` is excluded in some clones; check with `git check-ignore`). Use one row per child: number, branch, stage
(`ready`, `implementing`, `reviewing`, `pr-open`, `merged`, `done`, `blocked-on-user`),
implementer session id, reviewer session id, PR and one-line notes. Update it at every stage
change. After a context compaction, read the ledger and `gh` state before acting, never memory
alone. Sessions survive the orchestrator's compaction or move; local sub-agents do not: compare
the ledger with the remote branches and PRs, and restart any child whose work was never pushed.

## 2. Run each child

Run every ready child at once, each in its own session. There is no concurrency limit beyond file
ownership: sessions do not share CPU, disk or Docker with the orchestrator or with each other. When
a merge unblocks children, start them right away.

### Independent sessions

Start each implementer and each reviewer with the claude-code-remote `create_session` tool:

- `model`: the role's model id from the [defaults](#defaults-the-invocation-may-override).
- `source_url`: `https://github.com/aura-historia/backend`; `source_revision`: the epic branch for
  an implementer, the child branch for its reviewer. The branch must already exist on `origin`.
- `outcome_branch`: the child branch, so the session pushes there and nowhere else.
- `title` `#<child> implement (epic #<N>)` or `#<child> review (epic #<N>)`, and
  `tags` `["epic-<N>", "implementer"|"reviewer"]`.
- `prompt`: the full brief from [references/agent-briefs.md](references/agent-briefs.md). It
  must stand alone, because the session shares no files with you: it includes the environment
  notes, tells the agent to check out the child branch from the epic tip, to push it, and to send
  its report back with the claude-code-remote `send_message` tool to your session id (from
  `get_session` without an id).

Right after creating the first session, check that it started (`get_session`: no
`last_init_error`; `list_events`); a missing source branch fails every session at once. Reports
arrive as messages in your conversation. A session that ends without one does not report back:
schedule one safety-net check-in with `send_later` about 75 minutes out, and on it read
`get_session` (`status_bucket`) and `list_events` of each silent session. Archive a session with
`archive_session` once its child is merged or it failed to start. To send a reviewer a CI failure,
bot finding or merge conflict, use `send_message` to its session; it fixes the same branch.

### Fallback: local worktrees

Only when sessions cannot be created: run each role as an Agent-tool sub-agent in a worktree created
from the remote epic tip with `--no-track`
(`git worktree add --no-track ".claude/worktrees/<child>-<slug>" -b "<type>/#<child>-<slug>" "origin/<epic-branch>"`,
or `../wt-<child>` if `.claude/worktrees/` is not git-ignored). Then each worktree builds its own
Rust target on your machine: run about three children at once, and one at a time on a small
container (about 4 CPUs or a disk cap under 50 GB). The implementer commits locally without
pushing, and the reviewer pushes. Remove each worktree after its child merges.

### Environment notes for every brief

Each session is a fresh cloud container. Put these notes in every brief, so agents do not
rediscover them:

- `gh` and GraphQL: if `gh api graphql` is refused, agents use REST reads (`gh api repos/...`)
  and open PRs with the GitHub MCP tool `create_pull_request` or `gh api -X POST .../pulls`;
  `gh pr checks --watch` does not work, so CI is watched by you (step 4), not by the reviewer.
- Docker: if `docker info` fails and `dockerd` is installed, start it
  (`dockerd >/tmp/dockerd.log 2>&1 &`), so Docker-backed tests and the crawler dev binary run.
  It can stop when the container restarts; tell agents to check and restart it.
- Crawler dev verification without the desktop's `crawler_demo` state, `.env` or Vertex AI
  credentials: agents follow [crawler-dev-verify](../crawler-dev-verify/SKILL.md) with
  `CRAWLER_AUTHORING_MODE=manual` (no LLM, no credentials), swap in real shops that exercise the
  change and author URL patterns and schemas through the review console API. A session has no
  `crawler_demo`: it starts the [local database](../../../docs/crawler/operations.md#local-database)
  and creates the empty database once, which its run's copy migrates at startup. In the
  [fallback](#fallback-local-worktrees), each agent instead runs on its own copy of
  `crawler_demo` under the
  [parallel-runs precondition](../crawler-dev-verify/SKILL.md#preconditions), with its copy
  database and review-console port named in its brief. Never invent credentials.
- Host-specific test failures: record any test that fails only in the sessions' environment and
  passes in CI, and name it in later briefs, so agents do not chase it.
- Disk: a full workspace test run with debug info can fill a capped disk. Tell agents to build
  with `CARGO_PROFILE_DEV_DEBUG=0`.
- Commits: agents end commit messages with the attribution lines their own session gives and put
  model identifiers nowhere else.
- If the epic branch is behind `origin/develop` and a child needs a fix that landed there (for
  example, so the dev binary can fetch pages), fast-forward the epic branch to `origin/develop`
  when it is an ancestor (`git push origin origin/develop:refs/heads/<epic-branch>`); otherwise
  let the reviewer merge develop into the epic branch.

For each child:

1. **Branch.** Name the branch `<type>/#<child>-<short-slug>` (the `#` is required; quote it in
   shells). Types: `fix` for bugs, `feat` for new capabilities, `task` for everything else,
   including `improvement` issues. Tickets that share one owner of the same code can share one
   branch (named after the first ticket); the brief then names both.
2. **Implement.** Start an implementer session with the brief in
   [references/agent-briefs.md](references/agent-briefs.md#implementer), the child's scope, its
   share of the file ownership and the product-owner decisions that apply to it. It branches
   from the epic tip, implements, validates, commits, pushes the child branch and reports.
3. **Review and ship.** When it reports, start a fresh reviewer session on the child branch with
   the reviewer brief and the implementer's report. It reviews the child's diff against the issue
   and the repository rules, applies fixes as its own commits, pushes, opens the PR into the epic
   branch and reports. If the implementer reported a blocker or question that needs the user, ask
   the user before you start the review. If the implementer left an acceptance criterion unmet
   and called it a follow-up, tell the reviewer to meet it.
4. **Verify and merge.** Check the outcome yourself; an agent's report is not proof:

   ```sh
   gh pr view <pr> --json baseRefName,headRefName,mergeable,mergeStateStatus,statusCheckRollup,reviewDecision
   ```

   Merge only when the base is the epic branch, every required check passed on the latest head
   commit and the PR is mergeable: `gh pr merge <pr> --squash` (or REST:
   `gh api -X PUT repos/<owner>/<repo>/pulls/<pr>/merge -f merge_method=squash -f sha=<head>`).
   The repository squash-merges and deletes the head branch on merge. Where the harness offers
   PR activity subscriptions, subscribe to the child PR and wait for the CI events instead of
   polling, with one safety-net check-in about 50 minutes out.
   Read bot findings before merging: SonarCloud can pass its quality gate while reporting new
   issues (`curl -s 'https://sonarcloud.io/api/issues/search?componentKeys=<project>&pullRequest=<pr>&resolved=false'`).
   Send new MAJOR or higher findings the PR introduced to the child's reviewer session (or a fresh
   fixer session) with the fixer brief, then wait for CI on the new head.
5. **When it is not mergeable.** If the PR is behind or conflicts because a sibling merged first,
   send the conflict brief to the child's reviewer session, or start a fresh fixer session. It
   merges the epic branch into the child branch (no rebase or force-push on a pushed branch),
   resolves the conflicts, revalidates and pushes. Then go back to step 4.
6. **Clean up.** Archive the child's sessions, mark the child `merged` in the ledger, and
   recompute the ready set.

GitHub closes issues only for PRs merged into the default branch, so child issues stay open
until the epic PR lands. Do not close them by hand. The epic PR closes them.

## 3. Close the epic

When every child is `merged` or `done`:

1. Use the primary checkout for the final review if it has the epic branch checked out, since git
   refuses a second worktree on the same branch. Fast-forward it
   (`git pull --ff-only origin <epic-branch>`). If it has local changes to tracked files, ask.
   If the primary checkout is on another branch, create a worktree on the epic branch instead
   (`git worktree add ../wt-epic -B <epic-branch> origin/<epic-branch>`).
2. Start the final reviewer (Opus 5.5 by default) with the final-review brief, in a fresh session
   whose `source_revision` and `outcome_branch` are the epic branch, or as a sub-agent in that
   checkout. It brings the
   epic branch up to date with `origin/develop` by merge, reviews the whole
   `origin/develop...<epic-branch>` diff as one change, fixes what it finds, runs the broad
   validation from AGENTS.md, and pushes the fixes straight to the epic branch.
3. Open the PR from the epic branch to `develop`:
   - Title: `epic(#<N>): <what the epic delivers>`.
   - Body: a summary of what the epic delivers; one line per child with its merged PR; a
     `Closes #<child>` line for every child the epic implemented and `Closes #<N>`; the final
     review's fixes and validation; public-contract or operational notes (swagger, CHANGELOG,
     runbooks); and the findings left for follow-up tickets.
4. Have the user evaluate the follow-up findings, and create only the issues they approve. Never
   open a follow-up issue, or comment a finding onto an existing issue, without the user's
   permission, and never during the run: collect findings from the child reports, the child PRs
   and the final review in the ledger, and ask once, after every child is implemented and the
   final review is done. Deduplicate them first and check open issues, so each finding is
   presented as new or as covered by an existing issue. Present each one with the problem, its
   `path:line`, the source (child or final review), your recommendation (new issue, comment on
   #<existing>, fold into a planned ticket, or drop) and the epic it would belong to. Then, for
   the findings the user approves, and as the user decided: open one issue per coherent problem
   (`Follow-up from epic #<N> (#<epic PR>)`, problem with `path:line`, acceptance criteria,
   matching labels), or comment on the existing issue. Attach each new issue as a sub-issue of
   the open epic it belongs to
   (`gh api -X POST repos/<owner>/<repo>/issues/<epic>/sub_issues -F sub_issue_id=<issue id>`,
   using the issue's numeric `id`, not its number); leave it unattached if no epic fits. Then
   replace the follow-up list in the epic PR body with links to those issues and name the
   findings the user dropped.
5. Wait for CI on that PR. Have a fresh fixer handle failures caused by the epic. **Never merge
   it**; merging into `develop` is the user's call.
6. Archive the remaining child sessions (and remove any fallback worktrees), then tell the user
   the epic work is finished. Include the PR link, CI state, each child's PR, the follow-up
   issues the user approved, anything skipped or deferred, and open questions.

## When to stop and ask

The user expects this to run unattended, so ask only when a reasonable owner would. When you
ask, pause just the affected chain and keep the independent ones running. Ask when:

- an issue is ambiguous, contradicts the code or the epic, or would need a public-contract,
  persisted-format or cross-crate change it does not mention;
- implementer and reviewer disagree on a design point that the docs do not settle;
- CI fails for reasons unrelated to the change and still fails after one rerun of the failed
  jobs (`gh run rerun <run> --failed`), or a required check cannot run at all;
- a conflict resolution would change the behaviour of either side, not just combine them;
- a requested model, a permission or a credential is unavailable;
- a follow-up issue would be created or an existing issue commented on (always ask, once, at the
  end; see [step 3.4](#3-close-the-epic));
- the work would touch deployed services, live credentials or anything irreversible beyond
  pushing branches, opening PRs and merging child PRs into the epic branch.

Never work around one of these by narrowing the issue, skipping tests, disabling checks,
`--no-verify`, force-pushing shared branches or merging red CI.

## Keeping your context small

- Ask every agent for a short structured report (the briefs define it). Do not ask for diffs or
  logs. If you need a detail, ask the agent a targeted question or check `gh` state.
- Wait for agent reports (session messages or sub-agent notifications). Do not poll agents or
  loop on sleeps; the one safety-net check-in covers silent sessions. For CI, use the
  harness's PR monitoring where it exists; otherwise let the agent that owns the PR wait with one
  blocking `gh pr checks <pr> --watch` run in the background.
- When the user adds guidance mid-run (for example, how to run the crawler), forward it to the
  running agent with a message (`send_message` to its session) and add it to the ledger so later
  briefs carry it. When the user settles a default for good (models, where agents run, naming),
  update this skill on the epic branch so the next epic does not need to be told again.
- Put each child's facts in the ledger rather than repeating them in your own messages.
