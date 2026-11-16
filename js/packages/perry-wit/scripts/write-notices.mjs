// Write the license files the published package ships next to its code:
// a copy of the repository's LICENSE, and THIRD-PARTY-NOTICES.txt for the embedded Wasm compiler.
//
// Usage: node scripts/write-notices.mjs
import { chmod, copyFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { compilerNotices } from '../../../scripts/notices.mjs';

const PACKAGE_DIR = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const REPOSITORY_DIR = resolve(PACKAGE_DIR, '../../..');

const licenseTarget = join(PACKAGE_DIR, 'LICENSE');
await rm(licenseTarget, { force: true });
await copyFile(join(REPOSITORY_DIR, 'LICENSE'), licenseTarget);
await chmod(licenseTarget, 0o644);
const notices = await compilerNotices(REPOSITORY_DIR, 'perry-wit npm package and embedded Wasm compiler distribution');
await writeFile(join(PACKAGE_DIR, 'THIRD-PARTY-NOTICES.txt'), notices);
