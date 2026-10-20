'use strict';
const { github, hash } = require('./io.cjs');
const { owner, sameOwner } = require('./state.cjs');
function protection(ctx, get = github) {
  const environment = get(ctx, `environments/aws-${ctx.stage}`);
  const reviewers = environment.protection_rules?.find((rule) => rule.type === 'required_reviewers');
  if (!environment.id || !reviewers?.reviewers?.length || environment.can_admins_bypass === true)
    throw new Error(
      `aws-${ctx.stage} needs required reviewers and disabled administrator bypass before releasing.`,
    );
  return environment;
}
function approval(ctx, plan, get = github) {
  if (plan.run !== ctx.run || plan.attempt !== ctx.attempt)
    throw new Error(
      'Plan belongs to another run/attempt. Rerun all jobs to prepare and approve a fresh plan.',
    );
  const environment = protection(ctx, get);
  const reviews = get(ctx, `actions/runs/${ctx.run}/approvals`);
  const review = reviews.find(
    (item) => item.state === 'approved' && item.environments?.some((e) => e.id === environment.id),
  );
  if (!review?.user?.login)
    throw new Error('No protected release approval found for this run and environment.');
  return {
    environment: environment.name,
    actor: review.user.login,
    observedAt: new Date().toISOString(),
    run: ctx.run,
    attempt: ctx.attempt,
    plan: hash(plan),
    prerequisite: plan.requirements?.id || null,
  };
}
function canContinue(ctx, active, get = github) {
  if (sameOwner(active.owner, owner(ctx))) return;
  if (active.owner.repository !== ctx.repository)
    throw new Error('Release belongs to a different repository.');
  const prior = get(ctx, `actions/runs/${active.owner.run}/attempts/${active.owner.attempt}`);
  if (prior.status !== 'completed')
    throw new Error(
      'Previous owner is still active. Cancel it and confirm its attempt has completed before continuation. No lease expires automatically.',
    );
}
module.exports = { protection, approval, canContinue };
