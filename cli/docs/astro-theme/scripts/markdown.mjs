import GithubSlugger from 'github-slugger';
/** Preserve stable English and authored translated anchors without accepting raw HTML. */
export default function historicalMarkdown() {
  return tree => {
    const headings = [];
    const clean = node => {
      if (!node.children) return;
      node.children = node.children.filter(child => child.type !== 'html');
      if (node.type === 'heading') {
        const last = node.children.at(-1);
        const match = last?.type === 'text' && /\s*\{#([^\s{}]+)\}\s*$/.exec(last.value);
        if (match) {
          last.value = last.value.slice(0, match.index);
          node.data = { ...node.data, hProperties: { ...node.data?.hProperties, id: match[1] } };
        }
        headings.push(node);
      }
      for (const child of node.children) clean(child);
    };
    clean(tree);
    const reserved = new Set(headings.map(node => node.data?.hProperties?.id).filter(Boolean));
    const slugger = new GithubSlugger();
    const plain = node => node.value ?? node.alt ?? (node.children || []).map(plain).join('');
    for (const node of headings) {
      const natural = slugger.slug(plain(node).trim());
      const stable = node.data?.hProperties?.id;
      if (stable && natural && stable !== natural && !reserved.has(natural)) {
        reserved.add(natural);
        node.children.push({ type: 'anchorAlias', children: [], data: { hName: 'span', hProperties: { id: natural, 'aria-hidden': 'true' } } });
      }
    }
  };
}
