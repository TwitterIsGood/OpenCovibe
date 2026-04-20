<script lang="ts">
  import { onMount } from "svelte";
  import type { ProxyModelInfo } from "$lib/types";

  let {
    models = [],
    value = $bindable(""),
    label = "",
    onchange,
  }: {
    models: ProxyModelInfo[];
    value: string;
    label: string;
    onchange?: (value: string) => void;
  } = $props();

  let open = $state(false);
  let query = $state("");
  let wrapperEl: HTMLDivElement | undefined = $state();
  let buttonEl: HTMLButtonElement | undefined = $state();
  let panelStyle = $state("");
  let searchInput: HTMLInputElement | undefined = $state();

  function getPrefix(id: string): string {
    return id.split("-")[0];
  }

  type Group = { prefix: string; models: ProxyModelInfo[] };

  let groups = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? models.filter((m) => m.id.toLowerCase().includes(q))
      : models;
    const map = new Map<string, ProxyModelInfo[]>();
    for (const m of filtered) {
      const p = getPrefix(m.id);
      if (!map.has(p)) map.set(p, []);
      map.get(p)!.push(m);
    }
    const result: Group[] = [];
    for (const [prefix, items] of map) {
      items.sort((a, b) => b.id.localeCompare(a.id));
      result.push({ prefix, models: items });
    }
    result.sort((a, b) => a.prefix.localeCompare(b.prefix));
    return result;
  });

  let displayValue = $derived.by(() => {
    if (!value) return "";
    const found = models.find((m) => m.id === value);
    return found?.id ?? value;
  });

  function toggle() {
    open = !open;
    if (open) {
      query = "";
      updatePosition();
      // Focus search input after render
      setTimeout(() => searchInput?.focus(), 0);
    }
  }

  function updatePosition() {
    if (!buttonEl) return;
    const rect = buttonEl.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom;
    if (spaceBelow < 320) {
      panelStyle = `position:fixed;bottom:${window.innerHeight - rect.top + 4}px;left:${rect.left}px;z-index:50;max-height:${Math.min(rect.top - 8, 400)}px;`;
    } else {
      panelStyle = `position:fixed;top:${rect.bottom + 4}px;left:${rect.left}px;z-index:50;max-height:${Math.min(spaceBelow - 8, 400)}px;`;
    }
  }

  function select(id: string) {
    value = id;
    open = false;
    onchange?.(id);
  }

  function clear() {
    value = "";
    open = false;
    onchange?.("");
  }

  onMount(() => {
    function onClick(e: MouseEvent) {
      if (open && wrapperEl && !wrapperEl.contains(e.target as Node)) {
        open = false;
      }
    }
    function onKey(e: KeyboardEvent) {
      if (open && e.key === "Escape") open = false;
    }
    document.addEventListener("mousedown", onClick, true);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onClick, true);
      document.removeEventListener("keydown", onKey);
    };
  });
</script>

<div bind:this={wrapperEl} class="relative inline-flex">
  <button
    bind:this={buttonEl}
    class="inline-flex items-center gap-1 rounded-md border bg-background px-2 py-1 text-[11px] font-mono transition-colors hover:bg-accent"
    onclick={toggle}
  >
    <span class="text-muted-foreground text-[10px] font-sans font-medium">{label}</span>
    <span class="max-w-[160px] truncate">{displayValue || "—"}</span>
    <svg class="h-2.5 w-2.5 text-muted-foreground shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="m6 9 6 6 6-6" /></svg>
  </button>

  {#if open}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="w-80 rounded-md border bg-background shadow-lg"
      style="{panelStyle} overflow:hidden;display:flex;flex-direction:column;"
    >
      <!-- Search -->
      <div class="p-2 border-b shrink-0">
        <input
          bind:this={searchInput}
          bind:value={query}
          class="w-full rounded-sm border bg-background px-2 py-1 text-xs outline-none focus:ring-1 focus:ring-ring"
          placeholder="Search models..."
        />
      </div>

      <!-- Model list -->
      <div class="overflow-y-auto flex-1 p-1">
        {#each groups as group}
          <div class="px-2 pt-1.5 pb-0.5 text-[10px] font-semibold text-muted-foreground uppercase tracking-wide">
            {group.prefix} ({group.models.length})
          </div>
          {#each group.models as m}
            <button
              class="flex w-full items-center gap-1.5 rounded-sm px-2 py-1 text-[11px] font-mono hover:bg-accent transition-colors {value === m.id ? 'bg-accent font-medium' : ''}"
              onclick={() => select(m.id)}
            >
              {#if value === m.id}
                <svg class="h-3 w-3 text-primary shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M20 6 9 17l-5-5" /></svg>
              {:else}
                <span class="w-3 shrink-0"></span>
              {/if}
              <span class="truncate">{m.id}</span>
              {#if m.providerName}
                <span class="ml-auto text-[10px] text-muted-foreground/60 shrink-0">{m.providerName}</span>
              {/if}
            </button>
          {/each}
        {/each}

        {#if groups.length === 0}
          <p class="px-2 py-3 text-xs text-muted-foreground text-center">No models found</p>
        {/if}
      </div>

      <!-- Clear button -->
      {#if value}
        <div class="border-t p-1 shrink-0">
          <button
            class="flex w-full items-center gap-1.5 rounded-sm px-2 py-1 text-[11px] text-muted-foreground hover:bg-accent transition-colors"
            onclick={clear}
          >
            <svg class="h-3 w-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
            Clear selection
          </button>
        </div>
      {/if}
    </div>
  {/if}
</div>
