// Tests for claim.js. Run: node --test .github/scripts/
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const c = require('./claim.js');

const T0 = Date.parse('2026-11-17T10:00:00Z');
const H = 3600 * 1000;
const bot = (id, data) => ({ id, user: { type: 'Bot' }, body: `text\n\n${c.marker(data)}` });
const human = (id, body) => ({ id, user: { type: 'User' }, body });

test('parseCommand reads verbs, agents and hours', () => {
  assert.deepEqual(c.parseCommand('/claim agent-1'), { verb: 'claim', agent: 'agent-1', hours: 4 });
  assert.deepEqual(c.parseCommand('/claim agent-1 8h\nwhy'), { verb: 'claim', agent: 'agent-1', hours: 8 });
  assert.deepEqual(c.parseCommand('/renew a 12'), { verb: 'renew', agent: 'a', hours: 12 });
  assert.deepEqual(c.parseCommand('/unclaim a'), { verb: 'unclaim', agent: 'a' });
  assert.deepEqual(c.parseCommand('/unclaim force'), { verb: 'unclaim', force: true });
  assert.equal(c.parseCommand('please /claim a'), null);
  assert.equal(c.parseCommand('/claimed a'), null);
  assert.ok(c.parseCommand('/claim').error);
  assert.ok(c.parseCommand('/claim a 25').error);
  assert.ok(c.parseCommand('/claim a 0').error);
  assert.ok(c.parseCommand('/claim <x>').error);
  assert.ok(c.parseCommand('/unclaim').error);
});

test('readState takes the newest bot marker and ignores humans', () => {
  const list = [
    bot(1, { agent: 'a', until: '2026-11-17T12:00:00Z', pr: null }),
    human(2, c.marker({ agent: 'forged', until: '2099-01-01T00:00:00Z' })),
    bot(3, { agent: 'b', until: '2026-11-17T13:00:00Z', pr: 7 }),
    bot(4, { reply_to: 9, ok: false }),
  ];
  const s = c.readState(list);
  assert.equal(s.agent, 'b');
  assert.equal(s.pr, 7);
  assert.equal(s.commentId, 3);
  assert.equal(c.readState([]).agent, null);
});

test('a refused reply does not change the state', () => {
  const list = [bot(1, { agent: 'a', until: '2026-11-17T12:00:00Z', pr: null }), bot(2, { reply_to: 5, ok: false, agent: 'a', until: '2026-11-17T12:00:00Z', pr: null })];
  assert.equal(c.readState(list).agent, 'a');
});

test('claim: free issue is granted, held issue is refused, expired lock is free', () => {
  const free = { agent: null, until: null, pr: null };
  const r1 = c.decide({ verb: 'claim', agent: 'a', hours: 4 }, free, T0);
  assert.equal(r1.ok, true);
  assert.equal(Date.parse(r1.state.until), T0 + 4 * H);

  const r2 = c.decide({ verb: 'claim', agent: 'b', hours: 4 }, r1.state, T0 + H);
  assert.equal(r2.ok, false);
  assert.match(r2.message, /locked by `a`/);
  assert.match(r2.message, /3 h 0 min left/);

  const r3 = c.decide({ verb: 'claim', agent: 'b', hours: 2 }, r1.state, T0 + 4 * H + 1);
  assert.equal(r3.ok, true);
  assert.equal(r3.state.agent, 'b');
});

test('claim by the holder re-claims and keeps its pull request', () => {
  const s = { agent: 'a', until: new Date(T0 + H).toISOString(), pr: 12 };
  const r = c.decide({ verb: 'claim', agent: 'a', hours: 6 }, s, T0);
  assert.equal(r.ok, true);
  assert.equal(r.state.pr, 12);
  assert.equal(Date.parse(r.state.until), T0 + 6 * H);
});

test('renew only by the holder, also just after expiry', () => {
  const s = { agent: 'a', until: new Date(T0 + H).toISOString(), pr: null };
  assert.equal(c.decide({ verb: 'renew', agent: 'b', hours: 4 }, s, T0).ok, false);
  assert.equal(c.decide({ verb: 'renew', agent: 'a', hours: 4 }, s, T0).ok, true);
  assert.equal(c.decide({ verb: 'renew', agent: 'a', hours: 4 }, s, T0 + 2 * H).ok, true);
  assert.equal(c.decide({ verb: 'renew', agent: 'a', hours: 4 }, { agent: null }, T0).ok, false);
});

test('unclaim by the holder or force only', () => {
  const s = { agent: 'a', until: new Date(T0 + H).toISOString(), pr: null };
  assert.equal(c.decide({ verb: 'unclaim', agent: 'b' }, s, T0).ok, false);
  const r = c.decide({ verb: 'unclaim', agent: 'a' }, s, T0);
  assert.equal(r.ok, true);
  assert.equal(r.state.agent, null);
  assert.equal(c.decide({ verb: 'unclaim', force: true }, s, T0).state.agent, null);
});

test('parseRefs and parseAgent', () => {
  assert.deepEqual(c.parseRefs('Share sessions (#3)', 'Fixes #3\ncloses #10, see #99'), [3, 10]);
  assert.deepEqual(c.parseRefs('x', 'Claim: #4'), [4]);
  assert.deepEqual(c.parseRefs('no refs', ''), []);
  assert.equal(c.parseAgent('Fixes #1\nAgent: claude-7f3a\n'), 'claude-7f3a');
  assert.equal(c.parseAgent('no agent line'), null);
});

// A fake of the parts of the GitHub client the handlers use.
function fakeGithub(issues) {
  const calls = [];
  let nextId = 100;
  const rest = {
    issues: {
      listComments: (p) => issues[p.issue_number].comments,
      listForRepo: () => Object.entries(issues).filter(([, i]) => i.labels.includes(c.LABEL)).map(([n, i]) => ({ number: Number(n), ...i })),
      get: async (p) => ({ data: { number: p.issue_number, state: issues[p.issue_number].state } }),
      createComment: async (p) => {
        calls.push(['comment', p.issue_number, p.body]);
        (issues[p.issue_number] || (issues[p.issue_number] = { comments: [], labels: [], state: 'open' })).comments.push({ id: nextId++, user: { type: 'Bot' }, body: p.body });
      },
      updateComment: async (p) => {
        calls.push(['update', p.comment_id, p.body]);
        for (const i of Object.values(issues)) for (const cm of i.comments) if (cm.id === p.comment_id) cm.body = p.body;
      },
      addLabels: async (p) => { calls.push(['label+', p.issue_number]); issues[p.issue_number].labels.push(c.LABEL); },
      removeLabel: async (p) => { calls.push(['label-', p.issue_number]); const l = issues[p.issue_number].labels; l.splice(l.indexOf(c.LABEL), 1); },
    },
  };
  return { calls, github: { rest, paginate: async (fn, p) => fn(p) } };
}
const repo = { owner: 'o', repo: 'r' };

test('handleComment grants, then refuses a second agent, and labels the issue', async () => {
  const issues = { 3: { comments: [], labels: ['todo'], state: 'open' } };
  const { github, calls } = fakeGithub(issues);
  const ctx = (id, body) => ({ repo, payload: { issue: { number: 3, state: 'open' }, comment: { id, body, user: { type: 'User' } } } });
  await c.handleComment({ github, context: ctx(1, '/claim a'), now: T0 });
  assert.ok(issues[3].labels.includes('claimed'));
  await c.handleComment({ github, context: ctx(2, '/claim b'), now: T0 + H });
  const last = c.readMarker(calls.filter((x) => x[0] === 'comment').at(-1)[2]);
  assert.equal(last.reply_to, 2);
  assert.equal(last.ok, false);
  assert.equal(c.readState(issues[3].comments).agent, 'a');
});

test('handlePull claims for the Agent line and a push extends the same comment', async () => {
  const issues = { 5: { comments: [], labels: [], state: 'open' }, 9: { comments: [], labels: [], state: 'open' } };
  const { github, calls } = fakeGithub(issues);
  const pr = { number: 9, state: 'open', title: 'Thing (#5)', body: 'Agent: a', head: { ref: 'x/y' } };
  assert.deepEqual(await c.handlePull({ github, context: { repo, payload: { pull_request: pr } }, now: T0 }), ['#5 claimed']);
  assert.deepEqual(await c.handlePull({ github, context: { repo, payload: { pull_request: pr } }, now: T0 + 3 * H }), ['#5 extended']);
  assert.equal(calls.filter((x) => x[0] === 'update').length, 1);
  assert.equal(Date.parse(c.readState(issues[5].comments).until), T0 + 7 * H);
});

test('handlePull does not take an issue another agent holds', async () => {
  const issues = { 5: { comments: [bot(1, { agent: 'other', until: new Date(T0 + H).toISOString(), pr: null })], labels: ['claimed'], state: 'open' }, 9: { comments: [], labels: [], state: 'open' } };
  const { github } = fakeGithub(issues);
  const pr = { number: 9, state: 'open', title: 'Thing (#5)', body: '', head: { ref: 'mine' } };
  assert.deepEqual(await c.handlePull({ github, context: { repo, payload: { pull_request: pr } }, now: T0 }), ['#5 held by other']);
  assert.equal(c.readState(issues[5].comments).agent, 'other');
});

test('sweep releases expired locks only', async () => {
  const issues = {
    1: { comments: [bot(1, { agent: 'a', until: new Date(T0 - 1).toISOString(), pr: null })], labels: ['claimed'], state: 'open' },
    2: { comments: [bot(2, { agent: 'b', until: new Date(T0 + H).toISOString(), pr: null })], labels: ['claimed'], state: 'open' },
  };
  const { github } = fakeGithub(issues);
  assert.deepEqual(await c.sweep({ github, context: { repo }, now: T0 }), ['#1']);
  assert.equal(c.readState(issues[1].comments).agent, null);
  assert.ok(!issues[1].labels.includes('claimed'));
  assert.ok(issues[2].labels.includes('claimed'));
});
