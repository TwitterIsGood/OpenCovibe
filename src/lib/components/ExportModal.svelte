<script lang="ts">
  import { untrack } from "svelte";
  import { t } from "$lib/i18n/index.svelte";
  import * as api from "$lib/api";
  import type { TimelineEntry } from "$lib/types";
  import { buildExportHtml } from "$lib/utils/export-html";
  import { dbgWarn } from "$lib/utils/debug";
  import Modal from "./Modal.svelte";

  let {
    open = $bindable(false),
    runId = "",
    runName = "",
    timeline = [] as TimelineEntry[],
  }: {
    open: boolean;
    runId: string;
    runName?: string;
    timeline: TimelineEntry[];
  } = $props();

  type Format = "html" | "markdown" | "pdf";
  type RangeMode = "full" | "messages" | "time";
  type Phase = "configure" | "preview" | "exporting";

  let format = $state<Format>("html");
  let rangeMode = $state<RangeMode>("full");
  let phase = $state<Phase>("configure");
  let selectedIds = $state<Set<string>>(new Set());
  let dateFrom = $state("");
  let dateTo = $state("");
  let previewSrcdoc = $state("");
  let error = $state<string | null>(null);
  let anchorIndex = $state<number | null>(null);

  let messageEntries = $derived(
    timeline.filter((e) => e.kind === "user" || e.kind === "assistant"),
  );

  $effect(() => {
    if (!open) {
      untrack(() => {
        phase = "configure";
        format = "html";
        rangeMode = "full";
        selectedIds = new Set();
        anchorIndex = null;
        dateFrom = "";
        dateTo = "";
        previewSrcdoc = "";
        error = null;
      });
    }
  });

  function toggleAll() {
    if (selectedIds.size === messageEntries.length) {
      selectedIds = new Set();
    } else {
      selectedIds = new Set(messageEntries.map((e) => e.id));
    }
    anchorIndex = null;
  }

  function handleMessageClick(index: number, shiftKey: boolean) {
    if (shiftKey && anchorIndex !== null) {
      const from = Math.min(anchorIndex, index);
      const to = Math.max(anchorIndex, index);
      selectedIds = new Set(messageEntries.slice(from, to + 1).map((e) => e.id));
    } else {
      const id = messageEntries[index].id;
      const next = new Set(selectedIds);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      selectedIds = next;
      anchorIndex = index;
    }
  }

  function buildRange(): api.ExportRange {
    return { type: "full" };
  }

  async function loadPreview() {
    if (!runId) return;
    phase = "preview";
    error = null;

    try {
      const md = await api.exportConversationMarkdown(runId, { type: "full" });
      const title = runName || runId.slice(0, 8);
      const generatedAt = new Date().toLocaleString();
      previewSrcdoc = buildExportHtml(title, md, generatedAt);
    } catch (e) {
      dbgWarn("export-modal", "preview failed:", e);
      error = String(e);
    }
  }

  async function doExport() {
    if (!runId) return;
    phase = "exporting";
    error = null;

    try {
      const md = await api.exportConversationMarkdown(runId, buildRange());
      const title = runName || runId.slice(0, 8);
      const generatedAt = new Date().toLocaleString();

      if (format === "markdown") {
        const { save } = await import("@tauri-apps/plugin-dialog");
        const path = await save({
          defaultPath: `${title}.md`,
          filters: [{ name: "Markdown", extensions: ["md"] }],
        });
        if (path) {
          await api.writeExportFile(path, md);
        }
      } else if (format === "html") {
        const html = buildExportHtml(title, md, generatedAt);
        const { save } = await import("@tauri-apps/plugin-dialog");
        const path = await save({
          defaultPath: `${title}.html`,
          filters: [{ name: "HTML", extensions: ["html"] }],
        });
        if (path) {
          await api.writeExportFile(path, html);
        }
      } else if (format === "pdf") {
        const html = buildExportHtml(title, md, generatedAt);
        const iframe = document.createElement("iframe");
        iframe.style.position = "fixed";
        iframe.style.left = "-9999px";
        iframe.style.width = "800px";
        iframe.style.height = "600px";
        document.body.appendChild(iframe);
        iframe.contentDocument?.open();
        iframe.contentDocument?.write(html);
        iframe.contentDocument?.close();
        await new Promise((resolve) => setTimeout(resolve, 500));
        iframe.contentWindow?.print();
        setTimeout(() => document.body.removeChild(iframe), 5000);
      }
      open = false;
    } catch (e) {
      dbgWarn("export-modal", "export failed:", e);
      error = String(e);
      phase = "configure";
    }
  }

  function truncate(s: string, max: number): string {
    if (s.length <= max) return s;
    return s.slice(0, max) + "...";
  }

  function fmtTs(ts: string): string {
    try {
      return new Date(ts).toLocaleString();
    } catch {
      return ts;
    }
  }
</script>

<Modal bind:open title={t("export_modalTitle")} closeable={phase !== "exporting"}>
  {#if phase === "configure"}
    <!-- Format selection -->
    <div class="mb-4">
      <p class="mb-2 text-sm font-medium">{t("export_formatLabel")}</p>
      <div class="flex gap-2">
        {#each [["html", "export_formatHtml"], ["markdown", "export_formatMarkdown"], ["pdf", "export_formatPdf"]] as [val, key]}
          <button
            type="button"
            class="rounded-md border px-3 py-1.5 text-sm transition-colors
              {format === val ? 'border-primary bg-primary/10 text-primary font-medium' : 'hover:bg-muted'}"
            onclick={() => (format = val as Format)}
          >
            {t(key)}
          </button>
        {/each}
      </div>
    </div>

    <!-- Range selection -->
    <div class="mb-4">
      <p class="mb-2 text-sm font-medium">{t("export_rangeLabel")}</p>
      <div class="flex flex-col gap-1.5">
        {#each [["full", "export_rangeFull"], ["messages", "export_rangeMessages"], ["time", "export_rangeTime"]] as [val, key]}
          <label class="flex items-center gap-2 cursor-pointer">
            <input
              type="radio"
              name="exportRange"
              checked={rangeMode === val}
              onchange={() => (rangeMode = val as RangeMode)}
              class="accent-primary"
            />
            <span class="text-sm">{t(key)}</span>
          </label>
        {/each}
      </div>
    </div>

    <!-- Message selection -->
    {#if rangeMode === "messages"}
      <div class="mb-4">
        <div class="mb-2 flex items-center justify-between">
          <p class="text-sm text-muted-foreground">{t("export_selectMessages")}</p>
          <button
            type="button"
            class="text-xs text-muted-foreground hover:text-foreground transition-colors"
            onclick={toggleAll}
          >
            {selectedIds.size === messageEntries.length ? "Deselect all" : "Select all"}
          </button>
        </div>
        {#if messageEntries.length === 0}
          <p class="text-sm text-muted-foreground py-2">{t("export_noMessages")}</p>
        {:else}
          <div class="max-h-[200px] overflow-y-auto rounded-md border bg-muted/30 p-2">
            {#each messageEntries as entry, i}
              <label
                class="flex items-start gap-2 rounded px-1 py-1 hover:bg-muted/50 cursor-pointer"
                onclick={(e: MouseEvent) => {
                  e.preventDefault();
                  handleMessageClick(i, e.shiftKey);
                }}
              >
                <input
                  type="checkbox"
                  checked={selectedIds.has(entry.id)}
                  tabindex={-1}
                  class="pointer-events-none mt-0.5 rounded border-border"
                />
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-2">
                    <span class="text-xs font-medium {entry.kind === 'user' ? 'text-green-600' : 'text-blue-600'}">
                      {entry.kind === "user" ? "User" : "Assistant"}
                    </span>
                    <span class="text-xs text-muted-foreground">{fmtTs(entry.ts)}</span>
                  </div>
                  <p class="text-xs text-muted-foreground truncate">
                    {truncate(entry.content, 60)}
                  </p>
                </div>
              </label>
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    <!-- Time range -->
    {#if rangeMode === "time"}
      <div class="mb-4 flex gap-3">
        <div class="flex-1">
          <label class="block mb-1 text-xs text-muted-foreground">{t("export_dateFrom")}</label>
          <input
            type="date"
            bind:value={dateFrom}
            class="w-full rounded-md border bg-background px-2.5 py-1.5 text-sm"
          />
        </div>
        <div class="flex-1">
          <label class="block mb-1 text-xs text-muted-foreground">{t("export_dateTo")}</label>
          <input
            type="date"
            bind:value={dateTo}
            class="w-full rounded-md border bg-background px-2.5 py-1.5 text-sm"
          />
        </div>
      </div>
    {/if}

    <!-- PDF hint -->
    {#if format === "pdf"}
      <div class="mb-4 rounded-md bg-muted/50 px-3 py-2 text-xs text-muted-foreground">
        {t("export_pdfHint")}
      </div>
    {/if}

    {#if error}
      <div class="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
        {error}
      </div>
    {/if}

    <div class="flex justify-end gap-2">
      <button
        type="button"
        class="rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-muted"
        onclick={loadPreview}
        disabled={!runId}
      >
        {t("export_preview")}
      </button>
      <button
        type="button"
        class="rounded-md bg-primary px-3 py-1.5 text-sm text-primary-foreground transition-colors hover:bg-primary/90
          disabled:opacity-50 disabled:cursor-not-allowed"
        onclick={doExport}
        disabled={!runId || (rangeMode === "messages" && selectedIds.size === 0)}
      >
        {t("export_export")}
      </button>
    </div>

  {:else if phase === "preview"}
    <!-- Preview in sandboxed iframe -->
    {#if previewSrcdoc}
      <div class="mb-4 rounded-md border overflow-hidden" style="height: 50vh;">
        <iframe
          srcdoc={previewSrcdoc}
          sandbox="allow-same-origin"
          title="Export preview"
          style="width:100%; height:100%; border:none;"
        ></iframe>
      </div>
    {:else}
      <div class="flex items-center justify-center py-12">
        <div class="h-6 w-6 animate-spin rounded-full border-2 border-muted-foreground border-t-transparent"></div>
      </div>
    {/if}

    {#if error}
      <div class="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
        {error}
      </div>
    {/if}

    <div class="flex justify-end gap-2">
      <button
        type="button"
        class="rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-muted"
        onclick={() => (phase = "configure")}
      >
        {t("export_back")}
      </button>
      <button
        type="button"
        class="rounded-md bg-primary px-3 py-1.5 text-sm text-primary-foreground transition-colors hover:bg-primary/90"
        onclick={doExport}
      >
        {t("export_export")}
      </button>
    </div>

  {:else if phase === "exporting"}
    <div class="flex flex-col items-center gap-3 py-12">
      <div
        class="h-6 w-6 animate-spin rounded-full border-2 border-muted-foreground border-t-transparent"
      ></div>
      <p class="text-sm text-muted-foreground">{t("export_exporting")}</p>
    </div>
  {/if}
</Modal>
