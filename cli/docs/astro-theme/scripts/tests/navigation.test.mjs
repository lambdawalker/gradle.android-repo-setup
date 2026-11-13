import test from 'node:test';
import assert from 'node:assert/strict';
import { latestScope, compareVersions, destination, selectorGroups, currentPage, homeTargets, searchFilters } from '../navigation.mjs';
import { validate } from '../prepare.mjs';
import historicalMarkdown from '../markdown.mjs';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
const scope=(version,language='en',extra={})=>({language,module:'core',label:'Core',version,root:`/repo/${language}/core/${version}/`,pages:[{slug:'',title:'Start',href:`/repo/${language}/core/${version}/`},{slug:'quickstart',title:'Quickstart',href:`/repo/${language}/core/${version}/quickstart/`}],...extra});
test('latest chooses numeric stable release over prerelease and development',()=>{
 assert.equal(latestScope([scope('2.9.0'),scope('2.10.0'),scope('3.0.0-rc.1'),scope('development')]).version,'2.10.0');
 assert.ok(compareVersions(scope('2.0.0-rc.10'),scope('2.0.0-rc.2'))>0);
 assert.ok(compareVersions(scope('v2.0.0'),scope('2.0.0-rc.2'))>0);
 assert.equal(latestScope([scope('development')]).version,'development');
});
test('selectors preserve guides and supplied URLs, falling back to scope home',()=>{
 const current=scope('2.0.0'),translated=scope('2.0.0','es'),older=scope('1.0.0','en',{pages:[{slug:'overview',title:'Overview',href:'/repo/en/core/1.0.0/start/'}]});
 const groups=selectorGroups([current,translated,older],current,'quickstart');
 assert.equal(groups.Language[1].href,'/repo/es/core/2.0.0/quickstart/');
 assert.equal(groups.Version.find(v=>v.label==='1.0.0').href,'/repo/en/core/1.0.0/start/');
 assert.equal(destination(current,'quickstart'),current.pages[1].href);
});
test('page lookup handles deployment roots and exact routes',()=>{
 const pages=[{path:'en/core/1.0.0/quickstart'}];
 assert.equal(currentPage(pages,'/repo/en/core/1.0.0/quickstart/','/repo'),pages[0]);
 assert.equal(currentPage(pages,'/en/core/1.0.0/quickstart/'),pages[0]);
 assert.equal(currentPage(pages,'/repo/en/core/1.0.0/quickstart-extra/','/repo'),undefined);
});
test('home chooses English highest release for each module',()=>{
 const targets=homeTargets([scope('9.0.0','es'),scope('2.0.0'),scope('1.0.0'),scope('development')]);
 assert.equal(targets.length,1);assert.equal(targets[0].scope.version,'2.0.0');
});
test('markdown preserves heading IDs and ignores executable raw HTML',()=>{
 const processor=unified().use(remarkParse).use(historicalMarkdown);
 const tree=processor.runSync(processor.parse('## Stable heading {#stable-anchor}\n\n<script>alert(1)</script>\n\n```js\nconsole.log("data")\n```'));
 assert.equal(tree.children[0].data.hProperties.id,'stable-anchor');assert.equal(tree.children[0].children[0].value,'Stable heading');
 assert.ok(!tree.children.some(node=>node.type==='html'));assert.equal(tree.children.at(-1).type,'code');
});
test('input rejects traversal, duplicate routes, unsafe URLs and missing scope',()=>{
 const scopes=[scope('1.0.0')],site={title:'Docs'},page={path:'en/core/1.0.0/start',title:'Start',body:'# Heading',scope_index:0};
 assert.doesNotThrow(()=>validate([page],scopes,site));
 for(const path of ['../evil','/absolute','foo/../evil','foo.mdx','foo\\bar'])assert.throws(()=>validate([{...page,path}],scopes,site));
 assert.throws(()=>validate([page,page],scopes,site),/duplicate/);assert.throws(()=>validate([{...page,scope_index:9}],scopes,site));
 assert.throws(()=>validate([page],[scope('1.0.0','en',{root:'javascript:alert(1)'})],site));
 assert.throws(()=>validate([page],scopes,{...site,prefix:'//evil.example'}));
});

test('confirmed provider routes compare numeric release then canonical, Central, JitPack',()=>{
 assert.equal(latestScope([scope('0.1.1'),scope('0.2.0-maven-central')]).version,'0.2.0-maven-central');
 assert.equal(latestScope([scope('0.2.0-jitpack'),scope('0.2.0-maven-central')]).version,'0.2.0-maven-central');
 assert.equal(latestScope([scope('0.2.0-jitpack'),scope('0.2.0-maven-central'),scope('0.2.0')]).version,'0.2.0');
});
test('search always filters all three scope dimensions',()=>{
 assert.deepEqual(searchFilters(scope('1.2.0','es')),{language:'es',module:'core',version:'1.2.0'});
});
