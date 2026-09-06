import type { Element, Root, RootContent } from 'hast';

/** Reconnect local anchors after sanitization prefixes IDs against DOM clobbering. */
export function scopeReferences({ prefix }: { prefix: string }) {
  return (tree: Root) => {
    const elements: Element[] = [];
    const collect = (node: Root | RootContent) => {
      if (node.type === 'element') elements.push(node);
      if ('children' in node) node.children.forEach(collect);
    };
    collect(tree);
    const ids = new Set(elements.map((element) => element.properties.id).filter((id): id is string => typeof id === 'string'));
    const scoped = (id: string) => ids.has(prefix + id) ? prefix + id : id;
    for (const element of elements) {
      const props = element.properties;
      if (typeof props.href === 'string' && props.href.startsWith('#')) {
        try {
          const fragment = decodeURIComponent(props.href.slice(1));
          props.href = '#' + encodeURIComponent(scoped(fragment));
        } catch { /* Keep malformed fragments inert and unchanged. */ }
      }
      for (const key of ['ariaDescribedBy', 'ariaLabelledBy']) {
        const value = props[key];
        if (Array.isArray(value)) props[key] = value.map((id) => scoped(String(id)));
        else if (typeof value === 'string') props[key] = value.split(/\s+/).map(scoped).join(' ');
      }
    }
  };
}
