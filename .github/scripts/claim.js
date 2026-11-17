// Todo claims: an agent locks a todo issue for a limited time, so two agents never work on the
// same thing. Commands are issue comments (first line):
//
//   /claim <agent> [hours]    lock the issue for <agent> (default 4 h, max 24 h)
//   /renew <agent> [hours]    extend your own lock
//   /unclaim <agent>          release your own lock
//   /unclaim force            release whoever holds it (for a human)
//
// A pull request that names an issue (`(#N)` in the title, or `Fixes #N` / `Claim #N` in the body)
// claims it for the agent in its `Agent: <name>` body line, or for its branch name, and every push
// to that pull request extends the lock. A lock past its time is free for anyone; an hourly sweep
// also posts the release and removes the `claimed` label.
//
// The state lives in the bot's own comments: the newest bot comment carrying the marker below.
// Workflow runs are serialised per issue, so two claims at the same moment cannot both win.
'use strict';

const MARK = 'wc-claim';
const LABEL = 'claimed';
const DEFAULT_HOURS = 4;
const MAX_HOURS = 24;
const AGENT = /^[A-Za-z0-9][A-Za-z0-9._\/-]{0,59}$/;
const HOUR = 3600 * 1000;

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

function isBot(comment) {
  return Boolean(comment && comment.user && comment.user.type === 'Bot');
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
    if (!isBot(comments[i])) continue;
    const data = readMarker(comments[i].body);
    if (data && 'agent' in data) {
      return { agent: data.agent || null, until: data.until || null, pr: data.pr || null, commentId: comments[i].id };
    }
  }
  return { agent: null, until: null, pr: null, commentId: null };
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

async function handleComment({ github, context, now = Date.now() }) {
  const issue = context.payload.issue;
  const comment = context.payload.comment;
  if (!issue || issue.pull_request || !comment || isBot(comment)) return 'ignored';
  const cmd = parseCommand(comment.body);
  if (!cmd) return 'ignored';
  const { owner, repo } = context.repo;
  let result;
  if (cmd.error) {
    result = { ok: false, state: null, message: cmd.error };
  } else if (issue.state === 'closed' && cmd.verb !== 'unclaim') {
    result = { ok: false, state: null, message: 'Refused: the issue is closed.' };
  } else {
    const state = readState(await comments(github, owner, repo, issue.number));
    result = decide(cmd, state, now);
  }
  const data = { reply_to: comment.id, ok: result.ok };
  if (result.state) Object.assign(data, result.state);
  await github.rest.issues.createComment({ owner, repo, issue_number: issue.number, body: `${result.message}\n\n${marker(data)}` });
  if (result.ok && result.state) await setLabel(github, owner, repo, issue.number, Boolean(result.state.agent));
  return result.message;
}

async function handlePull({ github, context, now = Date.now() }) {
  const pr = context.payload.pull_request;
  if (!pr || pr.state !== 'open') return [];
  const { owner, repo } = context.repo;
  const agent = parseAgent(pr.body) || pr.head.ref;
  const out = [];
  for (const n of parseRefs(pr.title, pr.body)) {
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
      await github.rest.issues.createComment({
        owner, repo, issue_number: pr.number,
        body: `#${n} is locked by \`${state.agent}\` until ${stamp(Date.parse(state.until))}; this pull request does not hold it.`,
      });
      out.push(`#${n} held by ${state.agent}`);
      continue;
    }
    const until = now + DEFAULT_HOURS * HOUR;
    const next = { agent, until: new Date(until).toISOString(), pr: pr.number };
    const body = `Claimed by \`${agent}\` through #${pr.number} until ${stamp(until)}. Each push to #${pr.number} extends it by ${DEFAULT_HOURS} h.\n\n${marker(next)}`;
    if (state.agent === agent && state.pr === pr.number && state.commentId) {
      await github.rest.issues.updateComment({ owner, repo, comment_id: state.commentId, body });
      out.push(`#${n} extended`);
    } else {
      await github.rest.issues.createComment({ owner, repo, issue_number: n, body });
      await setLabel(github, owner, repo, n, true);
      out.push(`#${n} claimed`);
    }
  }
  return out;
}

async function sweep({ github, context, now = Date.now() }) {
  const { owner, repo } = context.repo;
  const issues = await github.paginate(github.rest.issues.listForRepo, { owner, repo, state: 'open', labels: LABEL, per_page: 100 });
  const released = [];
  for (const issue of issues) {
    if (issue.pull_request) continue;
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
  return released;
}

module.exports = {
  DEFAULT_HOURS, MAX_HOURS, LABEL,
  parseCommand, readState, isActive, decide, parseRefs, parseAgent, marker, readMarker,
  handleComment, handlePull, sweep,
};
