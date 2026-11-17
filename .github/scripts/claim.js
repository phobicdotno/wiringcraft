// Todo claims: an agent locks a todo issue for a limited time, so two agents never work on the
// same thing. Commands are issue comments (first line):
//
//   /claim <agent> [hours]    lock the issue for <agent> (default 4 h, max 24 h)
//   /renew <agent> [hours]    extend your own lock
//   /unclaim <agent>          release your own lock
//   /unclaim force            release whoever holds it (for a human)
//
// A pull request that names an issue (`(#N)` in the title, or `Fixes #N` / `Claim #N` in the body)
// claims it for the agent in its `Agent: <name>` body line, or for its branch name, until 4 h after
// the pull request's last push or edit. A lock past its time is free for anyone; an hourly sweep
// also posts the release and removes the `claimed` label.
//
// The state lives in the bot's own comments: the newest bot comment carrying the marker below.
// All runs of the claim workflow share one queue, so two claims at the same moment cannot both win,
// and every run first answers commands whose own run GitHub dropped from that queue (backlog).
'use strict';

const MARK = 'wc-claim';
const LABEL = 'claimed';
const DEFAULT_HOURS = 4;
const MAX_HOURS = 24;
const AGENT = /^[A-Za-z0-9][A-Za-z0-9._\/-]{0,59}$/;
const HOUR = 3600 * 1000;
const BACKLOG_HOURS = 6;

function parseHours(text) {
  if (text === undefined) return DEFAULT_HOURS;
  const m = /^(\d{1,2})h?$/i.exec(text);
  if (!m) return null;
  const h = Number(m[1]);
  return h >= 1 && h <= MAX_HOURS ? h : null;
}

/** The command on the first line of a comment, `{ error }` for a malformed one, or null. */
function parseCommand(body) {
  const line = String(body || '').trim().split(/\r?\n/)[0].trim();
  const m = /^\/(claim|renew|unclaim)\b(.*)$/.exec(line);
  if (!m) return null;
  const verb = m[1];
  const args = m[2].trim().split(/\s+/).filter(Boolean);
  if (verb === 'unclaim') {
    if (args.length !== 1) return { error: 'Usage: `/unclaim <agent>` or `/unclaim force`.' };
    if (args[0] === 'force') return { verb, force: true };
    if (!AGENT.test(args[0])) return { error: `\`${args[0]}\` is not a valid agent name.` };
    return { verb, agent: args[0] };
  }
  if (args.length < 1 || args.length > 2) {
    return { error: `Usage: \`/${verb} <agent> [hours]\` (hours 1 to ${MAX_HOURS}, default ${DEFAULT_HOURS}).` };
  }
  if (!AGENT.test(args[0])) return { error: `\`${args[0]}\` is not a valid agent name.` };
  const hours = parseHours(args[1]);
  if (hours === null) return { error: `Hours must be a whole number from 1 to ${MAX_HOURS}.` };
  return { verb, agent: args[0], hours };
}

function marker(data) {
  return `<!-- ${MARK} ${JSON.stringify(data)} -->`;
}

const CLAIM_BOT = 'github-actions[bot]';
// Who may use the commands and whose pull requests claim: people with write access.
const TRUSTED = new Set(['OWNER', 'MEMBER', 'COLLABORATOR']);

/** Any bot: never a source of commands. */
function isBot(comment) {
  return Boolean(comment && comment.user && comment.user.type === 'Bot');
}

/** Only the claim bot's own comments carry claim state; other bots could be asked to post a marker. */
function isClaimBot(comment) {
  return Boolean(comment && comment.user && comment.user.type === 'Bot' && comment.user.login === CLAIM_BOT);
}

function readMarker(body) {
  const m = new RegExp(`<!-- ${MARK} (\\{.*?\\}) -->`).exec(String(body || ''));
  if (!m) return null;
  try {
    return JSON.parse(m[1]);
  } catch {
    return null;
  }
}

/** The claim state from an issue's comments (oldest first): `{ agent, until, pr }`. */
function readState(comments) {
  for (let i = comments.length - 1; i >= 0; i--) {
    if (!isClaimBot(comments[i])) continue;
    const data = readMarker(comments[i].body);
    if (data && 'agent' in data) {
      return { agent: data.agent || null, until: data.until || null, pr: data.pr || null, commentId: comments[i].id, kind: data.kind || null };
    }
  }
  return { agent: null, until: null, pr: null, commentId: null, kind: null };
}

function isActive(state, now) {
  return Boolean(state.agent && state.until && Date.parse(state.until) > now);
}

function stamp(ms) {
  return new Date(ms).toISOString().slice(0, 16).replace('T', ' ') + ' UTC';
}

function left(state, now) {
  const mins = Math.max(0, Math.round((Date.parse(state.until) - now) / 60000));
  return mins >= 60 ? `${Math.floor(mins / 60)} h ${mins % 60} min` : `${mins} min`;
}

/**
 * What a command does to a claim. Returns `{ ok, state, message }`; `state` is the new state, or the
 * old one when nothing changes.
 */
function decide(cmd, state, now) {
  const keep = { agent: state.agent, until: state.until, pr: state.pr };
  const held = isActive(state, now);
  if (cmd.verb === 'claim') {
    if (held && state.agent !== cmd.agent) {
      return { ok: false, state: keep, message: `Refused: locked by \`${state.agent}\` until ${stamp(Date.parse(state.until))} (${left(state, now)} left).` };
    }
    const until = now + cmd.hours * HOUR;
    const pr = state.agent === cmd.agent ? state.pr : null;
    return {
      ok: true,
      state: { agent: cmd.agent, until: new Date(until).toISOString(), pr },
      message: `Claimed by \`${cmd.agent}\` until ${stamp(until)} (${cmd.hours} h). Renew with \`/renew ${cmd.agent}\`, release with \`/unclaim ${cmd.agent}\`.`,
    };
  }
  if (cmd.verb === 'renew') {
    if (!state.agent || state.agent !== cmd.agent) {
      const who = held ? `it is locked by \`${state.agent}\`` : 'it is not claimed by you';
      return { ok: false, state: keep, message: `Refused: ${who}. Claim it with \`/claim ${cmd.agent}\`.` };
    }
    const until = now + cmd.hours * HOUR;
    return {
      ok: true,
      state: { agent: cmd.agent, until: new Date(until).toISOString(), pr: state.pr },
      message: `Renewed for \`${cmd.agent}\` until ${stamp(until)} (${cmd.hours} h).`,
    };
  }
  // unclaim
  if (!state.agent) return { ok: true, state: keep, message: 'Not claimed; nothing to release.' };
  if (!cmd.force && state.agent !== cmd.agent) {
    return { ok: false, state: keep, message: `Refused: claimed by \`${state.agent}\`, not \`${cmd.agent}\`. A human can use \`/unclaim force\`.` };
  }
  return { ok: true, state: { agent: null, until: null, pr: null }, message: `Released (was \`${state.agent}\`). Free to claim.` };
}

/** Issue numbers a pull request names: `(#N)` in the title, `Fixes #N` or `Claim #N` in the body. */
function parseRefs(title, body) {
  const refs = new Set();
  for (const m of String(title || '').matchAll(/\(#(\d+)\)/g)) refs.add(Number(m[1]));
  const words = /\b(?:fix|fixes|fixed|close|closes|closed|resolve|resolves|resolved|claim|claims)\s*:?\s+#(\d+)/gi;
  for (const m of String(body || '').matchAll(words)) refs.add(Number(m[1]));
  return [...refs].sort((a, b) => a - b).slice(0, 5);
}

/** The `Agent: <name>` line in a pull request body, or null. */
function parseAgent(body) {
  const m = /^\s*Agent\s*:\s*(\S+)\s*$/im.exec(String(body || ''));
  return m && AGENT.test(m[1]) ? m[1] : null;
}

async function comments(github, owner, repo, issue_number) {
  return github.paginate(github.rest.issues.listComments, { owner, repo, issue_number, per_page: 100 });
}

async function setLabel(github, owner, repo, issue_number, on) {
  try {
    if (on) await github.rest.issues.addLabels({ owner, repo, issue_number, labels: [LABEL] });
    else await github.rest.issues.removeLabel({ owner, repo, issue_number, name: LABEL });
  } catch (e) {
    if (e.status !== 404) throw e;
  }
}

/** Answer one command comment on an issue. */
async function answer({ github, owner, repo, issue, comment, now }) {
  const cmd = parseCommand(comment.body);
  let result;
  if (!TRUSTED.has(comment.author_association)) {
    result = { ok: false, state: null, message: 'Refused: claims are for people with write access to the repository.' };
  } else if (cmd.error) {
    result = { ok: false, state: null, message: cmd.error };
  } else if (issue.state === 'closed' && cmd.verb !== 'unclaim') {
    result = { ok: false, state: null, message: 'Refused: the issue is closed.' };
  } else {
    const state = readState(await comments(github, owner, repo, issue.number));
    result = decide(cmd, state, now);
  }
  // Only a granted command records state: a refusal must not copy (and so shadow) the state it saw.
  const data = { reply_to: comment.id, ok: result.ok };
  if (result.ok && result.state) Object.assign(data, result.state);
  await github.rest.issues.createComment({ owner, repo, issue_number: issue.number, body: `${result.message}\n\n${marker(data)}` });
  if (result.ok && result.state) await setLabel(github, owner, repo, issue.number, Boolean(result.state.agent));
  return `#${issue.number}: ${result.message}`;
}

/**
 * Answer every command from the last hours that has no bot reply yet, oldest first. Every run does
 * this first: GitHub keeps only one pending run per concurrency group and cancels the others, so a
 * command whose own run was cancelled is answered by the next run.
 */
async function backlog({ github, context, now = Date.now() }) {
  const { owner, repo } = context.repo;
  const since = new Date(now - BACKLOG_HOURS * HOUR).toISOString();
  // `since` filters on the last edit; a command counts only when it was also created in the window,
  // so editing an old command never runs it again. Its reply is newer than the command, so it is
  // in the window too.
  const all = await github.paginate(github.rest.issues.listCommentsForRepo, { owner, repo, since, sort: 'created', direction: 'asc', per_page: 100 });
  const answered = new Set();
  for (const c of all) {
    const data = isClaimBot(c) ? readMarker(c.body) : null;
    if (data && data.reply_to) answered.add(data.reply_to);
  }
  const commands = all
    .filter((c) => !isBot(c) && !/\/pull\/\d+/.test(c.html_url || '') && Date.parse(c.created_at || 0) >= Date.parse(since) && parseCommand(c.body))
    .sort((a, b) => a.id - b.id);
  const out = [];
  const touched = new Set();
  for (const comment of commands) {
    const m = /\/issues\/(\d+)$/.exec(comment.issue_url || '');
    if (!m) continue;
    touched.add(Number(m[1]));
    if (answered.has(comment.id)) continue;
    const issue = (await github.rest.issues.get({ owner, repo, issue_number: Number(m[1]) })).data;
    if (issue.pull_request) continue;
    out.push(await answer({ github, owner, repo, issue, comment, now }));
  }
  // The label follows the claim state, also when setting it failed in an earlier run.
  for (const n of touched) {
    const state = readState(await comments(github, owner, repo, n));
    await setLabel(github, owner, repo, n, isActive(state, now));
  }
  return out;
}

async function handleComment({ github, context, now = Date.now() }) {
  return backlog({ github, context, now });
}

/** Post a note on a pull request once: skipped when a bot note with the same key is there. */
async function noteOnce(github, owner, repo, number, key, text) {
  const tag = `<!-- ${MARK}-note ${key} -->`;
  if ((await comments(github, owner, repo, number)).some((c) => isClaimBot(c) && String(c.body).includes(tag))) return;
  await github.rest.issues.createComment({ owner, repo, issue_number: number, body: `${text}\n\n${tag}` });
}

/**
 * The claims of one open pull request. A pull request's claim lasts until 4 h after its last
 * activity (`updated_at`: pushes, edits), so the result does not depend on which event or run
 * computes it, and an event GitHub dropped from the queue loses nothing.
 */
async function claimForPull({ github, owner, repo, pr, now }) {
  const refs = parseRefs(pr.title, pr.body);
  if (!refs.length) return [];
  if (!TRUSTED.has(pr.author_association)) return refs.map((n) => `#${n} author without write access`);
  const agent = parseAgent(pr.body) || (AGENT.test(pr.head.ref) ? pr.head.ref : null);
  if (!agent) {
    await noteOnce(github, owner, repo, pr.number, 'no-agent',
      `The branch name \`${pr.head.ref}\` cannot be an agent name; add a line \`Agent: <name>\` to the description to claim ${refs.map((n) => `#${n}`).join(', ')}.`);
    return refs.map((n) => `#${n} no agent`);
  }
  const until = (Date.parse(pr.updated_at || '') || now) + DEFAULT_HOURS * HOUR;
  if (until <= now) return refs.map((n) => `#${n} pull request idle`);
  const out = [];
  for (const n of refs) {
    let issue;
    try {
      issue = (await github.rest.issues.get({ owner, repo, issue_number: n })).data;
    } catch (e) {
      if (e.status === 404) continue;
      throw e;
    }
    if (issue.pull_request || issue.state !== 'open') continue;
    const state = readState(await comments(github, owner, repo, n));
    if (isActive(state, now) && state.agent !== agent) {
      await noteOnce(github, owner, repo, pr.number, `held-${n}-${state.agent}`,
        `#${n} is locked by \`${state.agent}\` until ${stamp(Date.parse(state.until))}; this pull request does not hold it.`);
      out.push(`#${n} held by ${state.agent}`);
      continue;
    }
    const mine = state.kind === 'pr' && state.agent === agent && state.pr === pr.number && state.commentId;
    // The pull request only ever lengthens the agent's lock: a longer claim it already holds stays.
    if (state.agent === agent && isActive(state, now) && Date.parse(state.until) >= until) {
      await setLabel(github, owner, repo, n, true);
      out.push(`#${n} held`);
      continue;
    }
    const next = { agent, until: new Date(until).toISOString(), pr: pr.number, kind: 'pr' };
    const body = `Claimed by \`${agent}\` through #${pr.number} until ${stamp(until)}. Each push to #${pr.number} extends it to ${DEFAULT_HOURS} h after the push.\n\n${marker(next)}`;
    // Extend by editing our own pull-request claim comment, never a reply to a command (the
    // command line waits for that reply).
    if (mine) {
      await github.rest.issues.updateComment({ owner, repo, comment_id: state.commentId, body });
      out.push(`#${n} extended`);
    } else {
      await github.rest.issues.createComment({ owner, repo, issue_number: n, body });
      out.push(`#${n} claimed`);
    }
    await setLabel(github, owner, repo, n, true);
  }
  return out;
}

/** Reconcile the claims of every open pull request (each run does this). */
async function pulls({ github, context, now = Date.now() }) {
  const { owner, repo } = context.repo;
  const open = await github.paginate(github.rest.pulls.list, { owner, repo, state: 'open', per_page: 100 });
  const out = [];
  for (const pr of open) out.push(...(await claimForPull({ github, owner, repo, pr, now })).map((s) => `PR #${pr.number}: ${s}`));
  return out;
}

/** One pull request, from its event payload (kept for direct use and tests). */
async function handlePull({ github, context, now = Date.now() }) {
  const pr = context.payload.pull_request;
  if (!pr || pr.state !== 'open') return [];
  const { owner, repo } = context.repo;
  return claimForPull({ github, owner, repo, pr, now });
}

async function sweep({ github, context, now = Date.now() }) {
  const { owner, repo } = context.repo;
  const issues = await github.paginate(github.rest.issues.listForRepo, { owner, repo, state: 'open', labels: LABEL, per_page: 100 });
  const released = [];
  for (const issue of issues) {
    if (issue.pull_request) continue;
    if (issue.state === 'closed') continue;
    const state = readState(await comments(github, owner, repo, issue.number));
    if (isActive(state, now)) continue;
    if (state.agent) {
      await github.rest.issues.createComment({
        owner, repo, issue_number: issue.number,
        body: `Lock of \`${state.agent}\` expired at ${stamp(Date.parse(state.until))}. Free to claim.\n\n${marker({ agent: null, until: null, pr: null })}`,
      });
    }
    await setLabel(github, owner, repo, issue.number, false);
    released.push(`#${issue.number}`);
  }
  // A closed issue needs no lock.
  const closed = await github.paginate(github.rest.issues.listForRepo, { owner, repo, state: 'closed', labels: LABEL, per_page: 100 });
  for (const issue of closed) {
    if (issue.pull_request) continue;
    await setLabel(github, owner, repo, issue.number, false);
    released.push(`#${issue.number} (closed)`);
  }
  return released;
}

module.exports = {
  DEFAULT_HOURS, MAX_HOURS, LABEL,
  parseCommand, readState, isActive, decide, parseRefs, parseAgent, marker, readMarker,
  handleComment, handlePull, sweep, backlog, pulls,
};
