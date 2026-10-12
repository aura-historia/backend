# Sources and optional external methodologies

The Aura skills are repository-specific instructions, not a vendored third-party plugin.
No external skill or tool is loaded at runtime. Current repository contracts are the source
of truth. The original preparation snapshot was develop@c77155fd7872461b41ccedbdd36492bf9595dbb1.

Client layout/metadata documentation checked on 2026-10-11:

- [Codex local skills and invocation policy](https://developers.openai.com/codex/skills)
- [Claude Code project skills and symlink folders](https://code.claude.com/docs/en/skills)

Optional further reading, never a runtime dependency or implicit installation request:

- [Trail of Bits Rust review](https://github.com/trailofbits/skills/tree/main/plugins/rust-review)
- [Trail of Bits false-positive checking](https://github.com/trailofbits/skills/tree/main/plugins/fp-check)
- [Trail of Bits differential review](https://github.com/trailofbits/skills/tree/main/plugins/differential-review)

These are discovery links, not pinned executable inputs. A separate human-approved upstream
integration must verify revision, license/attribution and all supporting agents/assets/tools.
Do not copy one manifest and pretend a complete plugin was installed. Do not fetch floating
main during an audit. This pack works from its local Markdown without any of these sources.
