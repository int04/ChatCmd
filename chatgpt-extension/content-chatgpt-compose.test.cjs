const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = readFileSync(join(__dirname, 'content-chatgpt-compose.js'), 'utf8');

function createEnvironment({ withInput = true, pasteHandled = false, resetInputOnChange = false } = {}) {
  class FakeFile {
    constructor(parts, name, options = {}) {
      this.name = name;
      this.type = options.type || '';
      this.size = parts.reduce((total, part) => total + (typeof part === 'string' ? part.length : part.byteLength || part.length || 0), 0);
    }
  }
  class FakeInput {
    constructor() {
      this.accept = 'image/*';
      this.multiple = true;
      this.disabled = false;
      this._files = [];
      this.events = [];
    }
    get files() { return this._files; }
    set files(value) { this._files = Array.from(value || []); }
    dispatchEvent(event) {
      this.events.push(event.type);
      if (resetInputOnChange && event.type === 'change') this._files = [];
      return true;
    }
  }
  class FakeDataTransfer {
    constructor() {
      const files = [];
      this.items = { add(file) { files.push(file); } };
      Object.defineProperty(this, 'files', { get: () => files });
    }
  }
  class FakeEvent {
    constructor(type) { this.type = type; }
  }
  class FakeClipboardEvent extends FakeEvent {
    constructor(type, options = {}) {
      super(type);
      this.clipboardData = options.clipboardData;
      this.defaultPrevented = false;
    }
    preventDefault() { this.defaultPrevented = true; }
  }

  const input = new FakeInput();
  const form = {
    textContent: '',
    querySelectorAll(selector) { return withInput && selector === 'input[type="file"]' ? [input] : []; },
  };
  const composer = {
    parentElement: form,
    closest(selector) { return selector === 'form' ? form : null; },
    dispatchEvent(event) {
      if (pasteHandled) event.preventDefault();
      return !event.defaultPrevented;
    },
  };
  const context = {
    console,
    File: FakeFile,
    HTMLInputElement: FakeInput,
    DataTransfer: FakeDataTransfer,
    Event: FakeEvent,
    ClipboardEvent: FakeClipboardEvent,
    Uint8Array,
    atob: (value) => Buffer.from(value, 'base64').toString('binary'),
    document: { body: { textContent: '' }, querySelectorAll: () => [] },
  };
  vm.createContext(context);
  vm.runInContext(source, context, { filename: 'content-chatgpt-compose.js' });
  return { context, composer, input, form };
}

function createBridge(context, composer, waitFor) {
  return context.ChatCmdComposerBridge.create({
    findComposer: () => composer,
    findSendButton: () => null,
    findStopButton: () => null,
    setComposerText() {},
    waitFor,
    delay: async () => {},
  });
}

const clipboardImage = [{
  name: 'clipboard-image-1.png',
  content: 'iVBORw0KGgo=',
  mimeType: 'image/png',
  encoding: 'base64',
}];

test('file-input attachment succeeds without requiring the image filename in DOM text', async () => {
  const { context, composer, input } = createEnvironment();
  const bridge = createBridge(context, composer, async () => { throw new Error('DOM confirmation should not be required'); });

  await bridge.attachFiles(composer, clipboardImage);

  assert.equal(input.files.length, 1);
  assert.equal(input.files[0].name, 'clipboard-image-1.png');
  assert.equal(input.files[0].type, 'image/png');
  assert.deepEqual(input.events, ['input', 'change']);
});

test('file-input proof survives ChatGPT resetting the picker during change handling', async () => {
  const { context, composer, input } = createEnvironment({ resetInputOnChange: true });
  const bridge = createBridge(context, composer, async () => { throw new Error('DOM confirmation should not be required'); });

  await bridge.attachFiles(composer, clipboardImage);

  assert.equal(input.files.length, 0);
  assert.deepEqual(input.events, ['input', 'change']);
});

test('paste fallback accepts ChatGPT preventing the paste event even when no filename is rendered', async () => {
  const { context, composer } = createEnvironment({ withInput: false, pasteHandled: true });
  const bridge = createBridge(context, composer, async () => { throw new Error('DOM confirmation should not be required'); });

  await bridge.attachFiles(composer, clipboardImage);
});

test('paste fallback can confirm a newly rendered attachment thumbnail without filename text', async () => {
  const { context, composer, form } = createEnvironment({ withInput: false });
  let attachmentNodes = [];
  form.querySelectorAll = (selector) => selector.includes('attachment') || selector.includes('blob:') ? attachmentNodes : [];
  const bridge = createBridge(context, composer, async (factory) => {
    attachmentNodes = [{}];
    const value = factory();
    if (!value) throw new Error('attachment UI was not detected');
    return value;
  });

  await bridge.attachFiles(composer, clipboardImage);
});
