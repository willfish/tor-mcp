const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { test } = require('node:test');
const { runInNewContext } = require('node:vm');
const page = readFileSync(`${__dirname}/../src/page.js`, 'utf8');

function field(value, type = 'text') {
  return {
    localName: 'input', value, outerHTML: '<input>',
    getClientRects: () => [{}],
    getAttribute: () => null,
    matches: selector => selector === 'input[type=password]' ? type === 'password'
      : selector === 'input[type=file]' ? type === 'file'
      : selector === 'input,textarea,select' || selector.includes('a[href]'),
    focus() { throw new Error('preflight must not focus the element'); },
  };
}
function extract(elements, args = ['snapshot', false]) {
  return runInNewContext(`(function() { ${page} }).apply(null, args)`, {
    args,
    document: { title: 'Fixture', querySelectorAll: () => elements },
    location: { href: 'https://example.com/' },
    getComputedStyle: () => ({ visibility: 'visible' }),
  });
}

test('snapshot bounds field values and reports value truncation', () => {
  const result = extract([field('x'.repeat(100_000)), field('short')]);
  assert.equal(result.nodes[0].value.length, 1000);
  assert.equal(result.nodes[0].value_truncated, true);
  assert.equal(result.nodes[1].value, 'short');
  assert.equal(result.nodes[1].value_truncated, false);
});

test('snapshot omits password values', () => {
  const result = extract([field('fixture-password', 'password')]);
  assert.equal(result.nodes[0].value, undefined);
  assert.equal(result.nodes[0].value_truncated, false);
});

test('snapshot reports additional matching nodes, not merely a full page', () => {
  assert.equal(extract(Array.from({ length: 200 }, () => field(''))).truncated, false);
  const result = extract(Array.from({ length: 201 }, () => field('')));
  assert.equal(result.nodes.length, 200);
  assert.equal(result.truncated, true);
});

test('fill preflight accepts editable fields without focusing or mutating', () => {
  const element = field('unchanged');
  assert.equal(extract([], ['fill_check', element]), null);
  assert.equal(element.value, 'unchanged');
});

test('fill preflight and fill reject file and non-editable targets before focus', () => {
  for (const operation of ['fill_check', 'fill']) {
    assert.throws(() => extract([], [operation, field('', 'file'), 'unused']), /upload_disabled/);
    const element = { ...field(''), localName: 'div', matches: () => false };
    assert.throws(() => extract([], [operation, element, 'unused']), /fill_target_not_editable/);
  }
});
