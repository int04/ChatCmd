import fs from 'node:fs';
import path from 'node:path';
import JavaScriptObfuscator from 'javascript-obfuscator';

const targets = process.argv.slice(2);
if (targets.length === 0) {
  console.error('Usage: node scripts/obfuscate-build.mjs <file-or-directory> [...]');
  process.exit(1);
}

const options = {
  compact: true,
  controlFlowFlattening: true,
  controlFlowFlatteningThreshold: 0.75,
  deadCodeInjection: true,
  deadCodeInjectionThreshold: 0.25,
  identifierNamesGenerator: 'hexadecimal',
  numbersToExpressions: true,
  renameGlobals: false,
  selfDefending: true,
  simplify: true,
  splitStrings: true,
  splitStringsChunkLength: 8,
  stringArray: true,
  stringArrayCallsTransform: true,
  stringArrayCallsTransformThreshold: 0.75,
  stringArrayEncoding: ['base64'],
  stringArrayIndexShift: true,
  stringArrayRotate: true,
  stringArrayShuffle: true,
  stringArrayThreshold: 1,
  transformObjectKeys: false,
  unicodeEscapeSequence: false,
};

function collectJsFiles(targetPath) {
  const stat = fs.statSync(targetPath);
  if (stat.isFile()) {
    return /\.(?:c|m)?js$/i.test(targetPath) ? [targetPath] : [];
  }

  const files = [];
  for (const entry of fs.readdirSync(targetPath, { withFileTypes: true })) {
    const fullPath = path.join(targetPath, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectJsFiles(fullPath));
    } else if (/\.(?:c|m)?js$/i.test(entry.name)) {
      files.push(fullPath);
    }
  }
  return files;
}

let processed = 0;
for (const input of targets) {
  const resolved = path.resolve(process.cwd(), input);
  if (!fs.existsSync(resolved)) {
    throw new Error(`Obfuscation target not found: ${resolved}`);
  }

  for (const file of collectJsFiles(resolved)) {
    const source = fs.readFileSync(file, 'utf8');
    const result = JavaScriptObfuscator.obfuscate(source, options);
    fs.writeFileSync(file, result.getObfuscatedCode(), 'utf8');
    processed += 1;
    console.log(`Obfuscated: ${path.relative(process.cwd(), file)}`);
  }
}

if (processed === 0) {
  throw new Error('No JavaScript files found to obfuscate.');
}

console.log(`Obfuscation completed: ${processed} JavaScript file(s).`);
