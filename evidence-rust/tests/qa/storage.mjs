import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import { randomBytes } from 'node:crypto';
import { mkdtemp, readFile, writeFile, stat, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';

const source=process.env.EVIDENCE_QA_AUTH_SOURCE;
assert.ok(source, 'An approved source-owned authority checkout is required');
const product=fileURLToPath(new URL('../../../',import.meta.url));
const archive=process.env.EVIDENCE_STORAGE_QA_ARCHIVE;
assert.ok(archive,'An explicit protected archive is required');
const pg=(await import(pathToFileURL(join(source,'central-auth/node_modules/pg/lib/index.js')).href)).default;
const {createCentralAuthGateway}=await import(pathToFileURL(join(source,'backend/central-auth-gateway.mjs')).href);
assert.equal((await stat(archive)).mode&0o077,0);
assert.ok(process.env.EVIDENCE_QA_REFERENCE,'Protected QA reference required');
assert.equal((await stat(process.env.EVIDENCE_QA_REFERENCE)).mode&0o077,0);
const reference=(await readFile(process.env.EVIDENCE_QA_REFERENCE,'utf8')).trim().replace(/^PG_QA_DATABASE_URL=/,'');
const url=new URL(reference);assert.equal(url.hostname,'127.0.0.1');assert.equal(url.pathname,'/auth_qa');url.port='15440';
const admin=new pg.Pool({connectionString:url.href});
const id='evidence19_'+randomBytes(12).toString('hex');
const content=id+'_content',authorityName=id+'_authority',restoreName=id+'_restore';
const owner=id+'_owner',runtime=id+'_runtime';
const temporary=await mkdtemp(join(tmpdir(),'evidence19-'));
const children=[];const secrets=[reference,url.href];const commands=[];const roles=[];const databases=[];
let identity,authority,authorityDB,contentDB;
const redact=input=>{let result=String(input);for(const secret of secrets)result=result.replaceAll(secret,'[REDACTED]');return result.replace(/postgres(?:ql)?:\/\/\S+/g,'[REDACTED PG URL]');};
function bounded(promise,ms=30000){let timer;return Promise.race([promise,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error('Exact lifecycle event timeout')),ms);timer.unref();})]).finally(()=>clearTimeout(timer));}
async function start(program,args,env,input,readiness){
  const child=spawn(program,args,{cwd:source,env:{...process.env,...env},stdio:['pipe','pipe','pipe']});
  child.lines=createInterface({input:child.stdout});child.logs='';child.exit=once(child,'close');children.push(child);
  const ready=readiness?once(child.lines,'line'):null;child.stderr.on('data',data=>{child.logs+=data;});
  if(input)child.stdin.end(JSON.stringify(input));
  return {child,ready:ready?await bounded(Promise.race([ready.then(([line])=>JSON.parse(line)),child.exit.then(()=>{throw new Error('Process exited before readiness');})])):null};
}
async function command(program,args,env={}){const child=spawn(program,args,{cwd:product,env:{...process.env,...env},stdio:['ignore','pipe','pipe']});const exit=once(child,'close');let output='';for(const stream of [child.stdout,child.stderr])stream.on('data',data=>{output+=data;});const result=await bounded(exit,900000);commands.push({program,args,exit:result,output:redact(output)});await writeFile(join(archive,'commands.json'),JSON.stringify(commands,null,2),{mode:0o600});assert.deepEqual(result,[0,null]);}
const catalog=async()=> (await admin.query('SELECT oid,datname,datdba,encoding,datcollate,datctype,datistemplate,datallowconn,datconnlimit,datacl FROM pg_database ORDER BY oid')).rows;
const before=await catalog();
let failure;
try {
  for(const role of [owner,runtime]){await admin.query(`CREATE ROLE ${role} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT`);roles.push(role);}
  for(const database of [content,authorityName,restoreName]){await admin.query(`CREATE DATABASE ${database} OWNER ${owner}`);databases.push(database);}
  const connection=database=>{const value=new URL(url);value.pathname='/'+database;secrets.push(value.href);return value.href;};
  const ownerURL=connection(content),authorityURL=connection(authorityName),restoreURL=connection(restoreName);
  const contentRoleURL=new URL(ownerURL);contentRoleURL.username=runtime;const contentURL=contentRoleURL.href;secrets.push(contentURL);
  const migrationURL=new URL(ownerURL);migrationURL.username=owner;const migrationOwnerURL=migrationURL.href;secrets.push(migrationOwnerURL);
  await writeFile(join(temporary,'owner-db'),migrationOwnerURL,{mode:0o600,flag:'wx'});
  await command('cargo',['build','--locked','--manifest-path','evidence-rust/Cargo.toml']);
  await command(join(product,'evidence-rust/target/debug/omo-evidence-storage'),['migrate'],{EVIDENCE_DATABASE_URL_FILE:join(temporary,'owner-db')});
  const roleDB=new pg.Pool({connectionString:ownerURL});
  await roleDB.query(`REVOKE CONNECT ON DATABASE ${content} FROM PUBLIC; GRANT CONNECT ON DATABASE ${content} TO ${runtime}; REVOKE CREATE ON SCHEMA public FROM PUBLIC; GRANT USAGE ON SCHEMA evidence TO ${runtime}; GRANT SELECT ON ALL TABLES IN SCHEMA evidence TO ${runtime}; GRANT INSERT,UPDATE ON evidence.evidence_entry TO ${runtime}; GRANT INSERT ON evidence.evidence_revision,evidence.evidence_asset,evidence.evidence_audit TO ${runtime}`);await roleDB.end();
  await writeFile(join(temporary,'qa.env'),'PG_QA_DATABASE_URL='+url.href,{mode:0o600});
  const identityProcess=await start('node',['auth-rust/tests/identity-fixture.mjs'],{AUTH_RUST_TEST_DATABASE_URL:url.href},null,true);identity=identityProcess.ready;identityProcess.child.identity=true;
  secrets.push(identity.credential,identity.secret,...Object.values(identity).flatMap(v=>v&&typeof v==='object'?[v.cookie,v.token].filter(Boolean):[]));
  const credentials=Object.fromEntries(['ci','evidence','gateway','csrf'].map(name=>[name,randomBytes(32).toString('base64url')]));
  secrets.push(...Object.values(credentials));for(const [name,value] of Object.entries(credentials))await writeFile(join(temporary,name),value,{mode:0o600,flag:'wx'});
  const authProcess=await start(join(source,'auth-rust/target/debug/examples/gateway_qa'),[],{}, {databaseURL:authorityURL,identity,ciFile:join(temporary,'ci'),evidenceFile:join(temporary,'evidence'),csrfFile:join(temporary,'csrf')},true);authority=authProcess.ready;
  secrets.push(authority.machineKey,authority.evidenceMachineKey);
  authorityDB=new pg.Pool({connectionString:authorityURL});
  await authorityDB.query(`UPDATE management.service_grant SET browser_scopes='["evidence:read","evidence:upload","evidence:publish","evidence:review"]' WHERE service_id='evidence'`);
  const adapter=createCentralAuthGateway({service:'evidence',privateAuthOrigin:authority.privateOrigin,serviceCredential:credentials.evidence,evidenceGatewaySecretFile:join(temporary,'gateway'),evidenceUploadToken:randomBytes(32).toString('base64url')});
  const origin='https://evidence.linalab.io';const login=await adapter.handle(new Request(origin+'/_linalab/auth/login'));assert.equal(login.status,303);
  const transaction=new URL(login.headers.get('location')).searchParams.get('request');const nonce=login.headers.get('set-cookie').split(';')[0];
  const form=await (await fetch(authority.publicOrigin+'/continue?request='+transaction,{headers:{cookie:identity.valid.cookie},signal:AbortSignal.timeout(10000)})).text();
  const field=(html,name)=>html.match(new RegExp(`name="${name}" value="([^"]+)"`))?.[1];
  const approved=await fetch(authority.publicOrigin+'/continue',{method:'POST',headers:{cookie:identity.valid.cookie,origin:'https://auth.linalab.io'},body:new URLSearchParams({request:transaction,csrf:field(form,'csrf')}),signal:AbortSignal.timeout(10000)});assert.equal(approved.status,200);
  const callback=await adapter.handle(new Request(origin+'/_linalab/auth/callback',{method:'POST',headers:{cookie:nonce,origin:'https://auth.linalab.io','content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({transaction,code:field(await approved.text(),'code')})}));assert.equal(callback.status,303);
  const browser=callback.headers.getSetCookie().find(v=>v.startsWith('__Host-linalab-evidence=')).split(';')[0].split('=')[1];secrets.push(browser);
  const fixture=join(temporary,'storage.json');await writeFile(fixture,JSON.stringify({databaseURL:contentURL,ownerDatabaseURL:migrationOwnerURL,restoreDatabaseURL:restoreURL,machineKey:authority.evidenceMachineKey,authorityOrigin:authority.privateOrigin,serviceSecret:credentials.evidence,browserHandle:browser,ownerId:authority.ownerId,authorityDatabaseURL:authorityURL}),{mode:0o600,flag:'wx'});
  await command('cargo',['test','--locked','--manifest-path','evidence-rust/Cargo.toml','--all-targets','--all-features'],{EVIDENCE_STORAGE_QA_FIXTURE:fixture,EVIDENCE_STORAGE_QA_ARCHIVE:archive});
  await command('cargo',['test','--locked','--manifest-path','evidence-rust/Cargo.toml','--doc']);
  contentDB=new pg.Pool({connectionString:contentURL});
  const tables=(await contentDB.query(`SELECT table_name FROM information_schema.tables WHERE table_schema='evidence' ORDER BY table_name`)).rows;
  await writeFile(join(archive,'content-tables.json'),JSON.stringify(tables,null,2),{mode:0o600});
} catch(error){failure=error;}
finally {
  for(const child of children.toReversed()){
    if(child.exitCode===null&&child.signalCode===null){if(child.identity)child.stdin.end('stop\n');else child.kill('SIGINT');}
    const exit=await bounded(child.exit);commands.push({process:child.identity?'source-identity':'source-rust-authority',exit,output:redact(child.logs)});child.lines.close();
    for(const secret of secrets)assert.equal(child.logs.includes(secret),false,'Secret appeared in child log');
  }
  if(contentDB)await contentDB.end();if(authorityDB)await authorityDB.end();
  for(const database of databases.toReversed())await admin.query(`DROP DATABASE ${database}`);
  for(const role of roles.toReversed())await admin.query(`DROP ROLE ${role}`);
  const after=await catalog();assert.deepEqual(after,before);
  await writeFile(join(archive,'cleanup.json'),JSON.stringify({before,after,ownedDatabases:databases,ownedRoles:roles,allOwnedAbsent:true},null,2),{mode:0o600});
  await writeFile(join(archive,'commands.json'),JSON.stringify(commands,null,2),{mode:0o600});
  await admin.end();await rm(temporary,{recursive:true,force:true});
}
if(failure){console.error(redact(failure.stack));process.exitCode=1;}else console.log('TASK19_STORAGE_QA_PASS authorReadyCommits=false currentPINIntegration=false');
