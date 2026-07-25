---
name: autonomously-push-changes
description: Use when asked to autonomously create GitHub issues, start issue work, push branches, or open pull requests in the Aura Historia backend. Covers branch names, issue/project fields, PR titles/descriptions, and state transitions.
---

# Autonomously Push Changes

Use this skill when the user asks you to work with GitHub, not just local files.

Never commit to `develop`. Never merge a pull request.

## First checks

- Read the relevant `AGENTS.md` chain first.
- Check git state before editing:
  - `git --no-pager status --short`
  - `git --no-pager branch --show-current`
- Do not overwrite or commit user changes you did not make.
- Check remote/issue/PR state before creating duplicates.
- Prefer `gh` for GitHub work.

## Branch naming

Use no `#` in branch names.

For normal issues:

```text
{task|feat|epic|docs|fix|deps}/{ISSUE_NUMBER}-short-title
```

For issues with a parent:

```text
{task|feat|epic|docs|fix|deps}/{PARENT_ISSUE_NUMBER}-short-title-parent/{task|feat|epic|docs|fix|deps}/{ISSUE_NUMBER}-short-title
```

Examples:

```text
epic/1341-hetzner-postgres-sequin-migration
docs/1341-hetzner-postgres-sequin-migration-parent/docs/1368-adr-draft
task/1341-hetzner-postgres-sequin-migration-parent/task/1371-api-runtime
fix/1450-token-validation
```

Rules:

- Use lowercase kebab-case.
- Keep short title stable and clear.
- Use `epic/` only for epic base branches.
- Use `docs/` for docs-only work.
- Use `deps/` for dependency-only work.
- Use `fix/` for bug fixes.
- Use `feat/` for user-visible feature work.
- Use `task/` for internal implementation work.

## Creating issues

When asked to create issues:

- Search existing issues first.
- Create small, parallelizable issues where possible.
- Link to parent epic when given.
- Add issues to project `Backend`.
- Set project state to `Todo` unless user says otherwise.
- Set sensible project priority.
- Set relevant labels and issue type.
- Keep issue body actionable and scoped.

Issue body template:

```md
Parent epic: #<parent>

## Priority

<Priority>

## Goal

<One clear outcome.>

## Scope

- <Included work>
- <Included work>

## Out of scope

- <Explicit non-work if useful>

## Must inspect

- `<path>`

## Acceptance criteria

- <Observable done state>
- <Tests/docs updated where needed>

## Validation

- `<command>`
```

Good issue description:

- says why, what, and done
- names key paths
- states dependencies/blockers
- states out-of-scope traps
- gives validation commands
- avoids vague words like “improve” without measurable outcome

## Starting work on issues

When starting an issue:

- Assign yourself if GitHub identity is available.
- Move issue project state to `In Progress`.
- Create branch from the correct base:
  - epic issue: from `develop` unless user gives other base
  - child issue: from parent epic branch when one exists
  - normal issue: from `develop` unless user gives other base
- Push branch early if a PR will be opened.
- Comment on the issue only when useful; avoid noise.

## Commits and pushing

- Commit focused changes with clear messages.
- Do not amend/rebase public work unless safe and useful.
- Push to origin branch.
- Keep generated temp files out of commits.
- If working tree contains user changes, commit only your files.

Commit message examples:

```text
docs(1368): draft Hetzner migration ADR
task(1371): add API runtime skeleton
fix(1450): handle missing token subject
deps(1460): bump sqlx
```

## Creating pull requests

PR title format:

```text
{task|feat|epic|docs|fix|deps}({ISSUE_NUMBER}): short-title
```

Examples:

```text
docs(1368): draft migration ADR
task(1371): add API runtime skeleton
fix(1450): reject malformed tokens
```

PR rules:

- Target the issue's base branch.
- Link issue in PR body.
- Move issue project state to `In Review`.
- Do not merge.
- Mark draft only when work is intentionally not ready for review.

PR body template:

```md
Closes #<issue>
Parent: #<parent-if-any>

## Intent

<Why this change exists.>

## Summary

- <Changed thing>
- <Changed thing>

## Breaking changes

- <None, or exact break/migration need.>

## Behavior impact

- <Runtime/API/storage/event impact.>

## Risk

- <Main risk and mitigation.>

## Validation

- `<command>`
```

Good PR description:

- says intent, not just file list
- lists breaking changes explicitly, even as `None`
- calls out API/storage/event/infra behavior changes
- includes validation actually run
- notes skipped validation with reason
- links issue and parent epic
- gives reviewer focus when helpful

## Project fields

When creating or moving issues, keep GitHub project `Backend` aligned:

- new issue: `Todo`
- started work: `In Progress`
- PR opened: `In Review`
- done/merged: do not move unless user asks or repo convention requires it

Set priority sensibly:

- `Critical`: blocks many tasks, architecture, data loss/security risk, release blocker
- `High`: core feature path or important migration slice
- `Medium`: useful implementation slice, not blocking most work
- `Low`: cleanup, docs polish, follow-up

Set labels/type from repo conventions. If unsure:

- inspect similar issues first
- choose minimal labels
- do not invent labels unless requested

## GitHub commands

Use `gh` safely:

```sh
gh issue list --repo aura-historia/backend --search "..."
gh issue create --repo aura-historia/backend --title "..." --body-file file.md
gh issue edit <number> --repo aura-historia/backend --add-label "..."
gh pr create --repo aura-historia/backend --base "..." --head "..." --title "..." --body-file file.md
gh pr view <number> --repo aura-historia/backend --json number,title,url,state
```

For project fields, inspect existing project item shape first and update with `gh project item-edit` or GraphQL as needed.

## Validation

Before final response:

- Confirm pushed branch and PR URL if created.
- Confirm issue state changes if made.
- Confirm validation commands run.
- Confirm PR not merged.
- Mention any uncommitted user changes left untouched.
