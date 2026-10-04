import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,stat,realpath} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
const product=fileURLToPath(new URL('../../../',import.meta.url));
const source=process.env.EVIDENCE_QA_AUTH_SOURCE,archive=process.env.EVIDENCE_STORAGE_QA_ARCHIVE;
assert.ok(source);assert.ok(archive);assert.equal((await stat(archive)).mode&0o077,0);
const reference=process.env.EVIDENCE_QA_REFERENCE;assert.ok(reference);assert.equal((await stat(reference)).mode&0o077,0);
const pg=(await import(pathToFileURL(join(source,'central-auth/node_modules/pg/lib/index.js')).href)).default;
const raw=(await readFile(reference,'utf8')).trim().replace(/^PG_QA_DATABASE_URL=/,'');const url=new URL(raw);assert.equal(url.hostname,'127.0.0.1');assert.equal(url.pathname,'/auth_qa');url.port='15440';
const pool=new pg.Pool({connectionString:url.href});const catalog=async()=>(await pool.query('SELECT oid,datname,datdba FROM pg_database ORDER BY oid')).rows;
const before=await catalog();const commands=[];
async function command(program,args,cwd=product){const child=spawn(program,args,{cwd,env:{...process.env,AUTH_RUST_QA_FILE:reference,AUTH_RUST_QA_PORT:'15440'},stdio:['ignore','pipe','pipe']});const exit=once(child,'close');let output='';for(const stream of [child.stdout,child.stderr])stream.on('data',chunk=>output+=chunk);const result=await exit;output=output.replaceAll(raw,'[REDACTED PG URL]').replaceAll(url.href,'[REDACTED PG URL]');commands.push({program,args,exit:result,output,outputSha256:createHash('sha256').update(output).digest('hex')});await writeFile(join(archive,'commands.json'),JSON.stringify(commands,null,2),{mode:0o600});assert.deepEqual(result,[0,null]);return output.trim();}
try {
  // c7eb7b8 is an explicit CI-QA-only successor; its auth-rust tree is
  // identical to the reviewed 6c5158a authority. Verify Cargo's physical path,
  // not just an unrelated checkout supplied by the operator.
  assert.equal(await realpath(source),await realpath(join(product,'../../linalab-ci-wt/reliability-20261002')));
  const head=await command('git',['rev-parse','HEAD'],source);
  assert.ok(['6c5158a6e4118cf0a9cc6bb21fc5e738f0e628b0','c7eb7b81bb15f77e14433686b805304567b40ca0'].includes(head));
  assert.equal(await command('git',['rev-parse','HEAD:auth-rust'],source),'77459922ef4d0dc2914f29e2af0b87f859a6d8ae');
  await command('cargo',['build','--locked','--manifest-path','evidence-rust/Cargo.toml']);
  await command('cargo',['test','--locked','--manifest-path','evidence-rust/tests/author-source/Cargo.toml','--test','author_ready','--','--nocapture']);
} finally {
  const after=await catalog();for(const row of before.filter(row=>['template1','template0','postgres','auth_qa','portal_58914_1790961734460669000','portal_59699_1790961746544346000','publish_qa_4d91b5f441ee300770c71df5'].includes(row.datname)))assert.deepEqual(after.find(v=>v.oid===row.oid),row);
  // An interrupted test may leave only databases whose exact created name was
  // printed by this owned process. Never sweep prefixes or foreign catalog rows.
  const output=commands.map(command=>command.output).join('\n');
  const exactOwned=[...output.matchAll(/"ownedDatabase":"(evidence_author19_[a-z0-9_]+|evidence_content19_[a-z0-9_]+)"/g)].map(match=>match[1]);
  const cleaned=[];for(const name of new Set(exactOwned)){if(after.some(row=>row.datname===name)){await pool.query(`DROP DATABASE ${name}`);cleaned.push(name);}}
  await writeFile(join(archive,'catalog.json'),JSON.stringify({before,after,foreignCleanupNotPerformed:true},null,2),{mode:0o600});await pool.end();
}
console.log('AUTHOR_SOURCE_STORAGE_PASS');
