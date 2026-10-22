---
name: orchestrate-epic
description: "Own a GitHub epic in aura-historia/backend end to end as an orchestrator: read its child issues and their 'blocked by' relationships, implement each child on its own branch with a fresh sub-agent, have a second fresh sub-agent review and fix it, open a PR into the epic branch and merge it once CI is green, run independent chains in parallel, then review the whole epic and open (never merge) the PR into develop. Invoke explicitly, e.g. '/orchestrate-epic 2227' while the epic branch is checked out."
disable-model-invocation: true
argument-hint: "[epic issue number] [model overrides]"
---

# Orchestrate an epic

You are the owner of one epic. Your job is to plan, delegate, verify and merge; sub-agents write
and review the code. Your own context has to last for the whole epic, which can be a dozen child
issues and many hours of CI, so you never read diffs or run builds yourself. You read short agent
reports and `gh` state, and you keep a ledger on disk so a compacted context can resume.

## Defaults the invocation may override

| Role | Default model and effort |
|---|---|
| Child implementer | Opus 5.5, XHigh |
| Child reviewer | Opus 5.5, XHigh |
| Merge-conflict, CI or bot-finding fixer | Opus 5.5, XHigh |
| Final epic reviewer | Opus 5.5, XHigh |

In Claude Code, spawn each role with the Agent tool, `model: opus` and `effort: xhigh`. Use
exactly the models the user names instead. If the harness cannot select one of them, stop and
ask before substituting. The user chose them on purpose, and a silent fallback would invalidate
the review claims in every PR.

Every sub-agent is fresh: it starts with no context from this conversation and none from another
agent. A reviewer is never the agent that wrote the code. Independent review is the point of the
second agent, and a continued implementer would just re-approve its own work.

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
(`.claude/worktrees/` is excluded in some clones; check with `git check-ignore`). Use one row per child: number, branch, worktree, stage
(`ready`, `implementing`, `reviewing`, `pr-open`, `merged`, `done`, `blocked-on-user`), PR,
agent ids and one-line notes. Update it at every stage change. After a context compaction, read
the ledger and `gh` state before acting, never memory alone. After the session moves to another
machine (desktop to cloud), sub-agents do not survive: compare the ledger with the remote
branches and PRs, and restart any child whose work was never pushed from a fresh worktree.

## 2. Run each child

Run every ready child at once, up to about three concurrently by default. Each worktree builds its
own Rust target, so more than that mostly causes CPU, disk and Docker contention on one machine;
on a small container (about 4 CPUs or a disk cap under 50 GB) run one child at a time. The user
may raise or lower the limit. When a merge unblocks children, start them right away.

### Environment checks before the first agent

Check these once and pass the results into every brief, so agents do not rediscover them:

- `gh` and GraphQL: if `gh api graphql` is refused, agents use REST reads (`gh api repos/...`)
  and open PRs with the GitHub MCP tool `create_pull_request` or `gh api -X POST .../pulls`;
  `gh pr checks --watch` does not work, so CI is watched by you (step 4), not by the reviewer.
- Docker: if `docker info` fails and `dockerd` is installed, start it
  (`dockerd >/tmp/dockerd.log 2>&1 &`), so Docker-backed tests and the crawler dev binary run.
  It can stop when the container restarts; tell agents to check and restart it.
- Crawler dev verification without the desktop's `crawler_demo` state, `.env` or Vertex AI
  credentials: run the dev binary with `CRAWLER_AUTHORING_MODE=manual` (no LLM, no credentials),
  let it bootstrap a fresh `crawler_demo`, swap the demo shops in `src/crawler/src/bin/dev.rs`
  for real shops that exercise the change (marked TEMPORARY, never committed, reverted), author
  URL patterns and schemas through the review console API, and inspect the results. Never invent
  credentials.
- Host-specific test failures: record any test that fails only on this host and passes in CI, so
  agents do not chase it.
- Disk: a full workspace test run with debug info can fill a capped disk. Tell agents to build
  with `CARGO_PROFILE_DEV_DEBUG=0` and remove stale worktree `target/` directories.
- If the epic branch is behind `origin/develop` and a child needs a fix that landed there (for
  example, so the dev binary can fetch pages), fast-forward the epic branch to `origin/develop`
  when it is an ancestor (`git push origin origin/develop:refs/heads/<epic-branch>`); otherwise
  let the reviewer merge develop into the epic branch.

For each child:

1. **Branch and worktree.** Name the branch `<type>/#<child>-<short-slug>` (the `#` is required;
   quote it in shells). Types: `fix` for bugs, `feat` for new capabilities, `task` for everything
   else, including `improvement` issues. Create the worktree from the current remote epic tip.
   Use `--no-track` so the child branch does not track the epic branch; the reviewer pushes it
   with `git push -u origin <branch>`. If `.claude/worktrees/` is not git-ignored, put the
   worktree next to the clone (`../wt-<child>`) instead:

   ```sh
   git fetch origin
   git worktree add --no-track ".claude/worktrees/<child>-<slug>" -b "<type>/#<child>-<slug>" "origin/<epic-branch>"
   ```

2. **Implement.** Spawn a fresh implementer with the brief in
   [references/agent-briefs.md](references/agent-briefs.md#implementer). It implements, validates
   and commits locally, and does not push.
3. **Review and ship.** When it reports, spawn a fresh reviewer with the reviewer brief. It reviews
   the child's diff against the issue and the repository rules, applies fixes as its own commits,
   pushes, opens the PR into the epic branch and reports. If the implementer reported a blocker
   or question that needs the user, ask the user before you start the review. If the implementer
   left an acceptance criterion unmet and called it a follow-up, tell the reviewer to meet it.
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
   Have a fresh fixer address new MAJOR or higher findings the PR introduced, then wait for CI on
   the new head.
5. **When it is not mergeable.** If the PR is behind or conflicts because a sibling merged first,
   spawn a fresh fixer with the conflict brief. It merges the epic branch into the child branch
   (no rebase or force-push on a pushed branch), resolves the conflicts, revalidates, pushes and
   waits for CI. Then go back to step 4.
6. **Clean up.** Remove the worktree (`git worktree remove`), mark the child `merged` in the
   ledger, and recompute the ready set.

GitHub closes issues only for PRs merged into the default branch, so child issues stay open
until the epic PR lands. Do not close them by hand. The epic PR closes them.

## 3. Close the epic

When every child is `merged` or `done`:

1. Use the primary checkout for the final review if it has the epic branch checked out, since git
   refuses a second worktree on the same branch. Fast-forward it
   (`git pull --ff-only origin <epic-branch>`). If it has local changes to tracked files, ask.
   If the primary checkout is on another branch, create a worktree on the epic branch instead
   (`git worktree add ../wt-epic -B <epic-branch> origin/<epic-branch>`).
2. Spawn the final reviewer (Opus 5.5 XHigh by default) with the final-review brief. It brings the
   epic branch up to date with `origin/develop` by merge, reviews the whole
   `origin/develop...<epic-branch>` diff as one change, fixes what it finds, runs the broad
   validation from AGENTS.md, and pushes the fixes straight to the epic branch.
3. Open the PR from the epic branch to `develop`:
   - Title: `epic(#<N>): <what the epic delivers>`.
   - Body: a summary of what the epic delivers; one line per child with its merged PR; a
     `Closes #<child>` line for every child the epic implemented and `Closes #<N>`; the final
     review's fixes and validation; public-contract or operational notes (swagger, CHANGELOG,
     runbooks); and the findings left for follow-up tickets.
4. Wait for CI on that PR. Have a fresh fixer handle failures caused by the epic. **Never merge
   it**; merging into `develop` is the user's call.
5. Remove any remaining child worktrees, then tell the user the epic work is finished. Include the PR link, CI
   state, each child's PR, anything skipped or deferred, and open questions.

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
- the work would touch deployed services, live credentials or anything irreversible beyond
  pushing branches, opening PRs and merging child PRs into the epic branch.

Never work around one of these by narrowing the issue, skipping tests, disabling checks,
`--no-verify`, force-pushing shared branches or merging red CI.

## Keeping your context small

- Ask every agent for a short structured report (the briefs define it). Do not ask for diffs or
  logs. If you need a detail, ask the agent a targeted question or check `gh` state.
- Wait for agent completion notifications. Do not poll agents or loop on sleeps. For CI, use the
  harness's PR monitoring where it exists; otherwise let the agent that owns the PR wait with one
  blocking `gh pr checks <pr> --watch` run in the background.
- When the user adds guidance mid-run (for example, how to run the crawler), forward it to the
  running agent with a message and add it to the ledger so later briefs carry it.
- Put each child's facts in the ledger rather than repeating them in your own messages.
