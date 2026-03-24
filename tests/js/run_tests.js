#!/usr/bin/env node
'use strict';

// ---------------------------------------------------------------------------
// Minimal test runner for depflame JS tests.
// Loads the sample report, mocks the DOM, evals the JS files, runs assertions.
// ---------------------------------------------------------------------------

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '../..');
const FIXTURE = path.join(ROOT, 'tests/fixtures/sample_report.json');

// Load sample report.
const report = JSON.parse(fs.readFileSync(FIXTURE, 'utf8'));

// ---------------------------------------------------------------------------
// Lightweight DOM mock.
// ---------------------------------------------------------------------------

class MockElement {
  constructor(tag) {
    this.tag = tag;
    this._children = [];
    this._attrs = {};
    this._innerHTML = '';
    this.style = {};
    this.textContent = '';
    this.classList = {
      add() {},
      remove() {},
      toggle() {},
    };
  }
  setAttribute(k, v) { this._attrs[k] = String(v); }
  getAttribute(k) { return this._attrs[k] || null; }
  appendChild(c) { this._children.push(c); return c; }
  querySelector() { return null; }
  querySelectorAll() { return []; }
  addEventListener() {}
  get innerHTML() { return this._innerHTML; }
  set innerHTML(v) { this._innerHTML = v; }
  get parentNode() { return { insertBefore() {} }; }
}

const SVG_NS = 'http://www.w3.org/2000/svg';
const elements = {};

global.document = {
  getElementById(id) {
    if (!elements[id]) elements[id] = new MockElement('div');
    return elements[id];
  },
  querySelectorAll() { return []; },
  addEventListener() {},
  createElement(t) { return new MockElement(t); },
  createElementNS(ns, t) { return new MockElement(t); },
};

global.window = { __DEPFLAME_REPORT__: report };
global.MutationObserver = class { observe() {} };
global.prompt = () => null;
global.requestAnimationFrame = function(cb) { cb(); };
global.setTimeout = global.setTimeout;

// ---------------------------------------------------------------------------
// Load JS modules (order matters: flamegraph, features, content, report).
// ---------------------------------------------------------------------------

function loadJS(relPath) {
  const code = fs.readFileSync(path.join(ROOT, relPath), 'utf8');
  // Use indirect eval to execute in global scope so `var X = ...` creates globals.
  (0, eval)(code);
}

loadJS('src/js/flamegraph.js');
loadJS('src/js/features.js');
loadJS('src/js/content.js');
loadJS('src/js/report.js');

// ---------------------------------------------------------------------------
// Test harness.
// ---------------------------------------------------------------------------

let passed = 0;
let failed = 0;
const failures = [];

function assert(condition, message) {
  if (!condition) throw new Error('Assertion failed: ' + message);
}

function assertEquals(actual, expected, message) {
  if (actual !== expected) {
    throw new Error(
      (message || 'assertEquals') +
      '\n  expected: ' + JSON.stringify(expected) +
      '\n  actual:   ' + JSON.stringify(actual)
    );
  }
}

function assertContains(haystack, needle, message) {
  if (typeof haystack !== 'string' || haystack.indexOf(needle) === -1) {
    throw new Error(
      (message || 'assertContains') +
      '\n  expected to contain: ' + JSON.stringify(needle) +
      '\n  in: ' + JSON.stringify(String(haystack).substring(0, 200) + '...')
    );
  }
}

function test(name, fn) {
  try {
    fn();
    passed++;
    process.stdout.write('  \x1b[32m✓\x1b[0m ' + name + '\n');
  } catch (e) {
    failed++;
    failures.push({ name, error: e });
    process.stdout.write('  \x1b[31m✗\x1b[0m ' + name + '\n');
    process.stdout.write('    ' + e.message.split('\n').join('\n    ') + '\n');
  }
}

// ---------------------------------------------------------------------------
// Load and run test files.
// ---------------------------------------------------------------------------

// Make test helpers available globally for test files.
global.test = test;
global.assert = assert;
global.assertEquals = assertEquals;
global.assertContains = assertContains;
global.report = report;
global.elements = elements;
global.MockElement = MockElement;

function loadTests(relPath) {
  console.log('\n' + relPath + ':');
  const code = fs.readFileSync(path.join(ROOT, relPath), 'utf8');
  (0, eval)(code);
}

loadTests('tests/js/test_content.js');
loadTests('tests/js/test_flamegraph.js');
loadTests('tests/js/test_features.js');

// ---------------------------------------------------------------------------
// Summary.
// ---------------------------------------------------------------------------

console.log('\n' + (passed + failed) + ' tests, ' + passed + ' passed, ' + failed + ' failed');
if (failed > 0) {
  process.exit(1);
}
