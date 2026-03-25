<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";

  let {
    url = "",
    ready = false,
    pickMode = $bindable(false),
    onClose,
    onPick,
  }: {
    url: string;
    ready: boolean;
    pickMode: boolean;
    onClose: () => void;
    onPick: () => void;
  } = $props();
</script>

<div class="flex items-center gap-2 px-3 py-1.5 border-b border-border bg-muted/30 text-xs">
  <span class="text-muted-foreground">{t("preview_label")}</span>
  <span class="font-mono text-foreground truncate flex-1">{url}</span>

  <button
    onclick={onPick}
    disabled={!ready}
    class="px-2 py-0.5 rounded text-xs transition-colors {pickMode
      ? 'bg-primary text-primary-foreground'
      : 'bg-muted hover:bg-accent text-foreground'} {!ready ? 'opacity-50 cursor-not-allowed' : ''}"
  >
    {pickMode ? t("preview_picking") : t("preview_pick")}
  </button>
  <button
    onclick={onClose}
    class="px-2 py-0.5 rounded text-xs bg-muted hover:bg-accent text-foreground transition-colors"
  >
    {t("preview_close")}
  </button>
</div>
