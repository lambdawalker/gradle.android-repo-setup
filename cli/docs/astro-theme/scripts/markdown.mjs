/** Explicit Zola heading IDs become actual heading IDs without executing historical code. */
export default function historicalMarkdown() {
  return tree => {
    const walk = node => {
      if (!node.children) return;
      node.children = node.children.filter(child => child.type !== 'html');
      if (node.type === 'heading') {
        const last = node.children.at(-1);
        const match = last?.type === 'text' && /\s*\{#([^\s{}]+)\}\s*$/.exec(last.value);
        if (match) {
          last.value = last.value.slice(0, match.index);
          node.data = { ...node.data, hProperties: { ...node.data?.hProperties, id: match[1] } };
        }
      }
      for (const child of node.children) walk(child);
    };
    walk(tree);
  };
}
