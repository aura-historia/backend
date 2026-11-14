# Email templates

The MJML sources of every email the backend sends. CI compiles each `*.mjml` file and uploads
the HTML to the mail template bucket; the SES adapter
([`notification-email-aws`](../src/notification-email-aws)) fills it with Handlebars data in
strict mode.

## Design first

The [Aura Historia Redesign](https://claude.ai/artifact/QRhqGsVofXjs5c86d11mrV) design canvas is
the source of truth for how every email looks and reads. Its **E-Mail** page has one artboard per
template, named `Email · <template directory>` (for example `Email · content-notice/receipt`),
plus the `Email · Kit` artboard with the shared colours, type, actions and MJML build notes.

When you add or change a template:

1. **Change the design canvas first.** Add or edit the template's artboard on the E-Mail page,
   following the Kit and the neighbouring email artboards: the same header, body and footer
   bands, a `showVariables` tweak that swaps the sample copy for the template's Handlebars
   fields, and one artboard per variant a reader can receive (for example a listing and a
   whole-provider content notice receipt).
2. **Then mirror it in MJML** in every email language (`de`, `en`, `es`, `fr`, `it`), with the
   same structure, labels and variables. Draw copy in English on the canvas and translate it
   in the MJML.
3. **Keep them aligned.** A template change that is not on the canvas, or a canvas change that
   is not in the MJML, is unfinished. Mention the artboard in the pull request.

## Conventions

- One directory per template kind, one file per email language: `<area>/<kind>/<language>.mjml`.
  `template_mapping.rs` maps each template type to its directory, subject and sender name.
- Every template renders in Handlebars strict mode, so each variable it uses must be in the
  template data; the adapter's template tests render every file.
- Emails to people without an account (external emails) carry no name and no account or
  settings links ([event flow](../docs/events/flow.md#external-email-delivery)).
