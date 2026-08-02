# DOX

## Purpose

- Own project-local agent skills.
- Keep repeated backend workflows out of broad `AGENTS.md` files.

## Core Design

- Skills in `.agents/skills/` hold focused task playbooks.
- Repo `AGENTS.md` files still own durable rules and path contracts.

## Ownership

- This doc rule `.agents/**`.
- Skill `SKILL.md` rule its skill directory.

## Local Contracts

- Keep skill descriptions specific so agent loads them only when useful.
- Update skills when repo workflow, infra wiring, docs contract, or test flow changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Prefer short checklist skills over giant shared docs.

## Verification

- Check skill frontmatter has matching `name` and directory.

## Child DOX Index
