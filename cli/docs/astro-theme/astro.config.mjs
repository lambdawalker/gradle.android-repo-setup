import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import historicalMarkdown from './scripts/markdown.mjs';
import { unified } from '@astrojs/markdown-remark';
import { repositoryUrl } from './scripts/navigation.mjs';
const scopes = JSON.parse(readFileSync(new URL('./scopes.json', import.meta.url), 'utf8'));
const languages = [...new Set(scopes.map(scope => scope.language))];
const site = JSON.parse(readFileSync(new URL('./site.json', import.meta.url), 'utf8'));
export default defineConfig({
  site: site.base_url || undefined,
  base: site.prefix || '/',
  outDir: process.env.ANDROID_REPO_DOCS_OUTPUT ? resolve(process.env.ANDROID_REPO_DOCS_OUTPUT) : './dist',
  markdown: { processor: unified({ remarkPlugins: [historicalMarkdown] }) },
  integrations: [starlight({
    title: site.title,
    defaultLocale: languages.includes('en') ? 'en' : languages[0],
    locales: Object.fromEntries(languages.map(language => [language, { label: language, lang: language }])),
    sidebar: [],
    components: { LanguageSelect: './src/components/ScopeLanguageSelect.astro', Search: './src/components/ScopeSearch.astro', MarkdownContent: './src/components/ScopeMarkdownContent.astro', Head: './src/components/ScopeHead.astro', PageTitle: './src/components/ScopePageTitle.astro', Banner: './src/components/ScopeBanner.astro', Sidebar: './src/components/ScopeSidebar.astro' },
    customCss: ['./src/styles/docs.css'],
    ...(site.repository ? { social: [{ icon: 'github', label: 'Repository', href: repositoryUrl(site.repository) }] } : {}),
  })],
});
