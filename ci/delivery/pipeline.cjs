#!/usr/bin/env node
"use strict";
const { prepare } = require("./preparation.cjs");
const execution = require("./execution.cjs");
async function main(command = process.argv[2]) {
  if (command === "prepare") return prepare();
  if (["admit", "migrate", "verify"].includes(command))
    return execution[command]();
  throw new Error("Usage: pipeline.cjs prepare|admit|migrate|verify");
}
if (require.main === module)
  main()
    .then((result) =>
      console.log(
        JSON.stringify({
          status: "SUCCEEDED",
          ...(result.approval ? result.approval : {}),
        }),
      ),
    )
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
module.exports = { main };
