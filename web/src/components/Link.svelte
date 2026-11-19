<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAnchorAttributes } from "svelte/elements";
  import { navigate } from "../lib/router.svelte";

  // a real <a> so every browser affordance works (middle click,
  // modifiers, copy link); only plain left clicks are intercepted
  let {
    href,
    children,
    ...rest
  }: { href: string; children?: Snippet } & Omit<HTMLAnchorAttributes, "href" | "children"> =
    $props();

  function onclick(event: MouseEvent) {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const url = new URL(href ?? "", location.href);
    if (url.origin !== location.origin) return;
    event.preventDefault();
    navigate(url.pathname);
  }
</script>

<a {href} {onclick} {...rest}>{@render children?.()}</a>
