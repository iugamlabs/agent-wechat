const { test } = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { spawn } = require('node:child_process');
const path = require('node:path');

const contacts = [
  { username: 'friend', nickName: 'Alice', contactType: 'individual', localType: 1 },
  { username: 'stranger', nickName: 'Alice', contactType: 'individual', localType: 3 },
];

test('shared contact methods keep positional arguments and pass optional filters', async () => {
  const { WeChatClient } = await import('../packages/shared/dist/index.js');
  const original = global.fetch;
  const requests = [];
  global.fetch = async url => {
    requests.push(new URL(url));
    return new Response(JSON.stringify(contacts), { status: 200 });
  };
  try {
    const client = new WeChatClient({ baseUrl: 'http://fixture.invalid' });
    assert.deepEqual(await client.listContacts(12, 4), contacts);
    await client.listContacts(12, 4, 1);
    await client.findContacts('Alice & Bob', 3);
    await client.findContacts('Alice');
    assert.equal(requests[0].search, '?limit=12&offset=4');
    assert.equal(requests[1].searchParams.get('localType'), '1');
    assert.equal(requests[1].searchParams.get('offset'), '4');
    assert.equal(requests[2].pathname, '/api/contacts/find');
    assert.equal(requests[2].searchParams.get('name'), 'Alice & Bob');
    assert.equal(requests[2].searchParams.get('localType'), '3');
    assert.equal(requests[3].searchParams.has('localType'), false);
  } finally {
    global.fetch = original;
  }
});

function runCli(baseUrl, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [path.join(__dirname, '../packages/cli/dist/cli.js'),
      'contacts', ...args], { env: { ...process.env, AGENT_WECHAT_URL: baseUrl, AGENT_WECHAT_TOKEN: 'fixture-token' } });
    let stdout = '', stderr = '';
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; });
    child.on('error', reject);
    child.on('close', code => resolve({ code, stdout, stderr }));
  });
}

test('CLI lists and searches expose raw types and reject unsupported filters', async () => {
  const requests = [];
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, 'http://fixture.invalid');
    requests.push(url);
    const filter = url.searchParams.get('localType');
    response.setHeader('Content-Type', 'application/json');
    response.end(JSON.stringify(contacts.filter(c => filter === null || c.localType === Number(filter))));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const base = `http://127.0.0.1:${server.address().port}`;
    const list = await runCli(base, ['list', '--local-type', '1']);
    assert.equal(list.code, 0, list.stderr);
    assert.match(list.stdout, /LocalType/);
    assert.match(list.stdout, /individual\s+1\s+Alice/);
    assert.doesNotMatch(list.stdout, /stranger/);
    assert.equal(requests[0].searchParams.get('localType'), '1');
    const found = await runCli(base, ['find', 'Alice', '--local-type', '3']);
    assert.equal(found.code, 0, found.stderr);
    assert.match(found.stdout, /stranger: Alice \[individual, localType=3\]/);
    assert.equal(requests[1].pathname, '/api/contacts/find');
    assert.equal(requests[1].searchParams.get('localType'), '3');
    const json = await runCli(base, ['list', '--json']);
    assert.equal(json.code, 0, json.stderr);
    assert.deepEqual(JSON.parse(json.stdout), contacts);
    for (const args of [['list', '--local-type', '2'], ['find', 'Alice', '--local-type', 'unknown']]) {
      const invalid = await runCli(base, args);
      assert.notEqual(invalid.code, 0);
      assert.match(invalid.stderr, /Allowed choices are 1, 3, 5/);
    }
    assert.equal(requests.length, 3);
  } finally {
    await new Promise(resolve => server.close(resolve));
  }
});
