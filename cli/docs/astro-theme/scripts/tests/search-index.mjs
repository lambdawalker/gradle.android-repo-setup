import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
/** Run the actual generated Pagefind JavaScript and WASM without requiring a browser. */
export async function verifySearch(output, prefix) {
  const previousFetch=globalThis.fetch;
  const previousDocument=globalThis.document;
  globalThis.document={currentScript:null,querySelector:()=>({getAttribute:()=> 'es'})};
  globalThis.fetch=async input=>{
    const url=new URL(String(input),'https://fixture.invalid');
    const path=url.protocol==='file:'?decodeURI(url.pathname):output+url.pathname.slice(prefix.length);
    const bytes=await readFile(path);
    return new Response(bytes,{headers:{'Content-Type':path.endsWith('.wasm')?'application/wasm':'application/octet-stream'}});
  };
  let engine;
  try {
    engine=await import(pathToFileURL(output+'/pagefind/pagefind.js').href);
    await engine.options({baseUrl:prefix+'/'});
    const filters=await engine.filters();
    assert.equal(filters.version['1.10.0'],2,'production index has exact scope metadata');
    const query={language:'es',module:'core',version:'1.10.0'};
    const response=await engine.search('Install',{filters:query});
    assert.equal(response.results.length,2,'search excludes other versions');
    const pages=await Promise.all(response.results.map(result=>result.data()));
    assert.ok(pages.every(page=>page.url.startsWith(`${prefix}/es/core/1.10.0/`)),'search preserves language, version and base prefix');
    for(const override of [{module:'unrelated'},{language:'en'},{version:'missing'}]) {
      const empty=await engine.search('Install',{filters:{...query,...override}});
      assert.equal(empty.results.length,0,'every scope filter is enforced by the actual engine');
    }
  } finally {
    await engine?.destroy();
    globalThis.fetch=previousFetch;
    if(previousDocument===undefined)delete globalThis.document;else globalThis.document=previousDocument;
  }
}
