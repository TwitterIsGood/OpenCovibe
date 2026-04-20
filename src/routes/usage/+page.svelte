<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import * as api from "$lib/api";
  import type { UsageOverview, DailyAggregate, ProxyRequestLog, ProxyDayHealth, ProxyLogDistinctValues } from "$lib/types";
  import { formatCost, formatTokenCount } from "$lib/utils/format";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import Card from "$lib/components/Card.svelte";
  import HeatmapCalendar from "$lib/components/HeatmapCalendar.svelte";
  import StackedModelChart from "$lib/components/StackedModelChart.svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { fmtDate, fmtNumber } from "$lib/i18n/format";

  let data = $state<UsageOverview | null>(null);
  let loading = $state(true);
  let error = $state("");
  let selectedDays = $state<number | undefined>(undefined); // undefined = all
  let heatmapDaily = $state<DailyAggregate[] | null>(null);
  let heatmapRequestId = 0;

  /** "app" = OpenCovibe runs only, "global" = all Claude Code sessions */
  let scope = $state<"app" | "global">("global");

  /** Monotonic counter to discard stale responses on rapid tab switching. */
  let requestId = 0;

  /** Whether to show the delayed first-load message (full scan taking > 500ms). */
  let showFullScanMessage = $state(false);

  /** Whether cache clear + rescan is in progress. */
  let refreshing = $state(false);

  // ── Proxy tab state ──
  let mainTab = $state<"overview" | "proxy">("overview");
  let proxyLogs = $state<ProxyRequestLog[]>([]);
  let proxyTotal = $state(0);
  let proxyPage = $state(0);
  const PROXY_PAGE_SIZE = 50;
  let proxyHealth = $state<ProxyDayHealth[]>([]);
  let proxyFilterModel = $state<string | null>(null);
  let proxyFilterProvider = $state<string | null>(null);
  let proxyFilterValues = $state<ProxyLogDistinctValues>({ models: [], providers: [] });

  async function loadProxyData() {
    try {
      const [logsResp, health, filterValues] = await Promise.all([
        api.getProxyLogs(
          { model: proxyFilterModel, providerId: proxyFilterProvider, days: 30 },
          PROXY_PAGE_SIZE,
          proxyPage * PROXY_PAGE_SIZE,
        ),
        api.getProxyHealth(24),
        api.getProxyLogFilters(),
      ]);
      proxyLogs = logsResp.entries;
      proxyTotal = logsResp.total;
      proxyHealth = health;
      proxyFilterValues = filterValues;
    } catch (e) {
      dbgWarn("usage", "loadProxyData error", e);
    }
  }

  function proxyTotalTokens(log: ProxyRequestLog): number {
    return (log.inputTokens ?? 0) + (log.outputTokens ?? 0) + (log.thinkingTokens ?? 0) + (log.cacheReadTokens ?? 0) + (log.cacheCreationTokens ?? 0);
  }

  const DATE_RANGES = [
    { label: "1d", days: 1 },
    { label: "7d", days: 7 },
    { label: "30d", days: 30 },
    { label: "90d", days: 90 },
    { label: "All", days: undefined as number | undefined },
  ];

  // Default chart mode: "messages" for global, "cost" for app
  let chartMode = $state<"cost" | "tokens" | "messages" | "sessions">("messages");

  let maxDailyValue = $derived.by(() => {
    if (!data?.daily.length) return 1;
    if (chartMode === "cost") {
      return Math.max(...data.daily.map((d) => d.costUsd), 0.01);
    }
    if (chartMode === "messages") {
      return Math.max(...data.daily.map((d) => d.messageCount ?? 0), 1);
    }
    if (chartMode === "sessions") {
      return Math.max(...data.daily.map((d) => d.sessionCount ?? 0), 1);
    }
    return Math.max(...data.daily.map((d) => d.inputTokens + d.outputTokens), 1);
  });

  // Sort state for run history
  let sortCol = $state<"date" | "cost" | "tokens" | "turns">("date");
  let sortAsc = $state(false);

  let sortedRuns = $derived.by(() => {
    if (!data?.runs) return [];
    const runs = [...data.runs];
    runs.sort((a, b) => {
      let cmp = 0;
      switch (sortCol) {
        case "date":
          cmp = a.startedAt.localeCompare(b.startedAt);
          break;
        case "cost":
          cmp = a.totalCostUsd - b.totalCostUsd;
          break;
        case "tokens":
          cmp = a.inputTokens + a.outputTokens - (b.inputTokens + b.outputTokens);
          break;
        case "turns":
          cmp = a.numTurns - b.numTurns;
          break;
      }
      return sortAsc ? cmp : -cmp;
    });
    return runs;
  });

  function toggleSort(col: typeof sortCol) {
    if (sortCol === col) {
      sortAsc = !sortAsc;
    } else {
      sortCol = col;
      sortAsc = false;
    }
  }

  function sortIndicator(col: typeof sortCol): string {
    if (sortCol !== col) return "";
    return sortAsc ? " \u25B2" : " \u25BC";
  }

  async function loadData(days?: number) {
    const thisRequest = ++requestId;
    if (!data) loading = true; // Only show full spinner on initial load
    error = "";
    showFullScanMessage = false;

    // Delayed indicator: show message if full scan takes > 500ms
    const delayTimer = setTimeout(() => {
      if (thisRequest === requestId) {
        showFullScanMessage = true;
      }
    }, 500);

    try {
      let result: UsageOverview;
      if (scope === "global") {
        result = await api.getGlobalUsageOverview(days);
      } else {
        result = await api.getUsageOverview(days);
      }

      // Discard stale response if user switched tabs/scope while we were loading
      if (thisRequest !== requestId) {
        dbg("usage", "discarded stale response", { thisRequest, currentRequest: requestId });
        return;
      }

      data = result;
      dbg("usage", "loadData", {
        scope,
        days,
        scanMode: data?.scanMode,
        dailyLen: data?.daily.length,
        firstDaily: data?.daily[0],
        totalRuns: data?.totalRuns,
        byModelLen: data?.byModel.length,
      });
    } catch (e) {
      if (thisRequest !== requestId) return;
      error = String(e);
    } finally {
      clearTimeout(delayTimer);
      if (thisRequest === requestId) {
        loading = false;
        showFullScanMessage = false;
      }
    }
  }

  async function loadHeatmapData() {
    const token = ++heatmapRequestId;
    try {
      const result = await api.getHeatmapDaily(scope);
      if (token === heatmapRequestId) {
        heatmapDaily = result;
        dbg("usage", "heatmap loaded", { scope, days: result.length });
      } else {
        dbg("usage", "heatmap discarded stale", { token, current: heatmapRequestId });
      }
    } catch (e) {
      if (token === heatmapRequestId) {
        heatmapDaily = null;
        dbgWarn("usage", "heatmap load failed", e);
      }
    }
  }

  function selectRange(days: number | undefined) {
    selectedDays = days;
    loadData(days);
  }

  function selectScope(s: "app" | "global") {
    scope = s;
    // Reset chart mode if current mode isn't available for this scope
    if (s === "app" && (chartMode === "messages" || chartMode === "sessions")) {
      chartMode = "cost";
    }
    loadData(selectedDays);
    loadHeatmapData();
  }

  async function refreshCache() {
    if (refreshing) return;
    refreshing = true;
    try {
      await api.clearUsageCache();
      await Promise.all([loadData(selectedDays), loadHeatmapData()]);
    } finally {
      refreshing = false;
    }
  }

  function formatDate(isoStr: string): string {
    return fmtDate(isoStr);
  }

  function formatShortDate(dateStr: string): string {
    // dateStr is "YYYY-MM-DD"
    return dateStr.slice(5); // "MM-DD"
  }

  function getDailyValue(day: DailyAggregate): number {
    if (chartMode === "cost") return day.costUsd;
    if (chartMode === "messages") return day.messageCount ?? 0;
    if (chartMode === "sessions") return day.sessionCount ?? 0;
    return day.inputTokens + day.outputTokens;
  }

  function getDailyTooltip(day: DailyAggregate): string {
    const date = day.date;
    if (chartMode === "cost") return `${date}\n${formatCost(day.costUsd)}`;
    if (chartMode === "messages")
      return `${date}\n${t("usage_tooltipMessages", { count: fmtNumber(day.messageCount ?? 0) })}`;
    if (chartMode === "sessions")
      return `${date}\n${t("usage_tooltipSessions", { count: String(day.sessionCount ?? 0) })}`;
    return `${date}\n${t("usage_tooltipTokens", { count: formatTokenCount(day.inputTokens + day.outputTokens) })}`;
  }

  function formatAxisValue(v: number): string {
    if (chartMode === "cost") return formatCost(v);
    if (chartMode === "tokens") return formatTokenCount(v);
    if (v >= 1000) return `${(v / 1000).toFixed(1)}k`;
    return v.toFixed(0);
  }

  onMount(() => {
    loadData(selectedDays);
    loadHeatmapData();
  });
</script>

<div class="max-w-4xl mx-auto p-6 space-y-6 animate-slide-up">
  <!-- Header -->
  <div class="flex items-center gap-4">
    <div class="flex h-14 w-14 items-center justify-center rounded-2xl bg-emerald-500/10">
      <svg
        class="h-7 w-7 text-emerald-600 dark:text-emerald-400"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"><path d="M3 3v18h18" /><path d="m19 9-5 5-4-4-3 3" /></svg
      >
    </div>
    <div>
      <h1 class="text-2xl font-bold">{t("usage_title")}</h1>
      <p class="text-sm text-muted-foreground">{t("usage_subtitle")}</p>
    </div>
  </div>

  <!-- Main tab: Overview / Proxy -->
  <div class="flex gap-1 bg-muted/40 rounded-lg p-0.5 w-fit">
    <button
      class="px-3 py-1.5 text-xs font-medium rounded-md transition-colors
        {mainTab === 'overview'
        ? 'bg-background text-foreground shadow-sm'
        : 'text-muted-foreground hover:text-foreground'}"
      onclick={() => { mainTab = "overview"; }}
    >
      {t("usage_tabOverview")}
    </button>
    <button
      class="px-3 py-1.5 text-xs font-medium rounded-md transition-colors
        {mainTab === 'proxy'
        ? 'bg-background text-foreground shadow-sm'
        : 'text-muted-foreground hover:text-foreground'}"
      onclick={() => { mainTab = "proxy"; loadProxyData(); }}
    >
      {t("usage_tabProxy")}
    </button>
  </div>

  {#if mainTab === "overview"}
  <!-- Overview tab content -->
  <div class="flex items-center gap-4">
    <div class="flex gap-1 bg-muted/40 rounded-lg p-0.5">
      <button
        class="px-3 py-1.5 text-xs font-medium rounded-md transition-colors
          {scope === 'global'
          ? 'bg-background text-foreground shadow-sm'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => selectScope("global")}
      >
        {t("usage_scopeGlobal")}
      </button>
      <button
        class="px-3 py-1.5 text-xs font-medium rounded-md transition-colors
          {scope === 'app'
          ? 'bg-background text-foreground shadow-sm'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => selectScope("app")}
      >
        {t("usage_scopeApp")}
      </button>
    </div>

    <!-- Date range tabs -->
    <div class="flex gap-1">
      {#each DATE_RANGES as range}
        <button
          class="px-3 py-1.5 text-xs font-medium rounded-md transition-colors
            {selectedDays === range.days
            ? 'bg-primary text-primary-foreground'
            : 'bg-muted/50 text-muted-foreground hover:bg-muted'}"
          onclick={() => selectRange(range.days)}
        >
          {range.label}
        </button>
      {/each}
    </div>

    <!-- Refresh button (global scope only, stays in DOM to avoid layout shift) -->
    <button
      class="p-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-colors disabled:opacity-40 {scope !==
      'global'
        ? 'invisible'
        : ''}"
      title={t("usage_refreshTitle")}
      disabled={refreshing || scope !== "global"}
      onclick={refreshCache}
    >
      <svg
        class="h-4 w-4 {refreshing ? 'animate-spin' : ''}"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
        <path d="M3 3v5h5" />
        <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
        <path d="M16 21h5v-5" />
      </svg>
    </button>
  </div>

  {#if loading}
    <div class="flex flex-col items-center justify-center py-12 gap-3">
      <div
        class="h-6 w-6 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
      ></div>
      {#if showFullScanMessage}
        <p class="text-sm text-muted-foreground animate-fade-in">
          {t("usage_firstLoadMessage")}
        </p>
      {/if}
    </div>
  {:else if error}
    <div
      class="rounded-lg border border-destructive/50 bg-destructive/10 px-4 py-3 text-sm text-destructive"
    >
      {error}
    </div>
  {:else if data}
    <!-- Summary cards -->
    <div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
      <Card class="p-4 text-center">
        <p class="text-2xl font-bold">{formatCost(data.totalCostUsd)}</p>
        <p class="text-xs text-muted-foreground mt-1">{t("usage_totalCost")}</p>
      </Card>
      <Card class="p-4 text-center">
        <p class="text-2xl font-bold">{formatTokenCount(data.totalTokens)}</p>
        <p class="text-xs text-muted-foreground mt-1">{t("usage_totalTokens")}</p>
      </Card>
      <Card class="p-4 text-center">
        <p class="text-2xl font-bold">{data.totalRuns}</p>
        <p class="text-xs text-muted-foreground mt-1">
          {scope === "global" ? t("usage_sessions") : t("usage_runs")}
        </p>
      </Card>
      <Card class="p-4 text-center">
        {#if data.currentStreak > 0}
          <p class="text-2xl font-bold">
            {t("usage_currentStreak", { count: String(data.currentStreak) })}
          </p>
          <p class="text-xs text-muted-foreground mt-1">
            {t("usage_longestStreak", { count: String(data.longestStreak) })}
          </p>
        {:else}
          <p class="text-2xl font-bold">
            {t("usage_activeDays", { count: String(data.activeDays) })}
          </p>
          <p class="text-xs text-muted-foreground mt-1">
            {scope === "global" ? t("usage_avgCostSession") : t("usage_avgCostRun")}
          </p>
        {/if}
      </Card>
    </div>

    <!-- Activity Heatmap (always 52 weeks, independent of date filter) -->
    {#if heatmapDaily}
      <Card class="p-6 space-y-3">
        <div class="flex items-center justify-between">
          <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
            {t("usage_activityHeatmap")}
          </h2>
          <div class="flex gap-3 text-xs text-muted-foreground">
            {#if data.activeDays > 0}
              <span>{t("usage_activeDays", { count: String(data.activeDays) })}</span>
            {/if}
            {#if data.longestStreak > 0}
              <span>{t("usage_longestStreak", { count: String(data.longestStreak) })}</span>
            {/if}
          </div>
        </div>
        <HeatmapCalendar daily={heatmapDaily} metric={chartMode} />
      </Card>
    {/if}

    <!-- Daily trend chart -->
    <Card class="p-6 space-y-4">
      <div class="flex items-center justify-between">
        <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
          {t("usage_dailyTrend")}
        </h2>
        <div class="flex gap-1">
          <button
            class="px-2 py-0.5 text-[10px] font-medium rounded transition-colors
              {chartMode === 'cost'
              ? 'bg-primary/20 text-primary'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => (chartMode = "cost")}
          >
            {t("usage_chartCost")}
          </button>
          <button
            class="px-2 py-0.5 text-[10px] font-medium rounded transition-colors
              {chartMode === 'tokens'
              ? 'bg-primary/20 text-primary'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => (chartMode = "tokens")}
          >
            {t("usage_chartTokens")}
          </button>
          {#if scope === "global"}
            <button
              class="px-2 py-0.5 text-[10px] font-medium rounded transition-colors
                {chartMode === 'messages'
                ? 'bg-primary/20 text-primary'
                : 'text-muted-foreground hover:text-foreground'}"
              onclick={() => (chartMode = "messages")}
            >
              {t("usage_chartMessages")}
            </button>
            <button
              class="px-2 py-0.5 text-[10px] font-medium rounded transition-colors
                {chartMode === 'sessions'
                ? 'bg-primary/20 text-primary'
                : 'text-muted-foreground hover:text-foreground'}"
              onclick={() => (chartMode = "sessions")}
            >
              {t("usage_chartSessions")}
            </button>
          {/if}
        </div>
      </div>
      {#if data.daily.length > 0}
        {#if scope === "global" && chartMode === "tokens" && data.daily.some((d) => d.modelBreakdown)}
          <StackedModelChart daily={data.daily} />
        {:else}
          <div class="flex h-40">
            <!-- Y-axis labels -->
            <div
              class="flex flex-col justify-between items-end pr-2 text-[10px] text-muted-foreground tabular-nums shrink-0 py-0.5"
            >
              <span>{formatAxisValue(maxDailyValue)}</span>
              <span>{formatAxisValue(maxDailyValue / 2)}</span>
              <span>0</span>
            </div>
            <!-- Bars + X-axis -->
            <div class="flex-1 flex flex-col min-w-0">
              <div class="flex-1 flex gap-[2px] border-l border-b border-border/50 relative">
                <!-- 50% gridline -->
                <div
                  class="absolute inset-x-0 top-1/2 border-t border-border/30 pointer-events-none"
                ></div>
                {#each data.daily.slice(-30) as day}
                  {@const value = getDailyValue(day)}
                  {@const pct = Math.max((value / maxDailyValue) * 100, 2)}
                  <div
                    class="flex-1 min-w-0 flex items-end group cursor-default"
                    title={getDailyTooltip(day)}
                  >
                    <div
                      class="w-full rounded-t bg-primary/60 group-hover:bg-primary transition-colors"
                      style="height: {pct}%"
                    ></div>
                  </div>
                {/each}
              </div>
              <!-- X-axis date labels -->
              <div class="flex gap-[2px] mt-1">
                {#each data.daily.slice(-30) as day, i}
                  {@const showLabel =
                    data.daily.slice(-30).length <= 10 ||
                    i % Math.ceil(data.daily.slice(-30).length / 10) === 0}
                  <div class="flex-1 min-w-0 text-center">
                    {#if showLabel}
                      <span class="text-[10px] text-muted-foreground tabular-nums">
                        {formatShortDate(day.date)}
                      </span>
                    {/if}
                  </div>
                {/each}
              </div>
            </div>
          </div>
        {/if}
      {:else}
        <p class="text-sm text-muted-foreground">{t("usage_noDailyData")}</p>
      {/if}
    </Card>

    <!-- By Model -->
    <Card class="p-6 space-y-4">
      <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
        {t("usage_byModel")}
      </h2>
      {#if data.byModel.length > 0}
        <div class="overflow-x-auto">
          <table class="w-full text-sm">
            <thead>
              <tr class="text-xs text-muted-foreground border-b border-border">
                <th class="text-left py-2 font-medium">{t("usage_thModel")}</th>
                {#if scope === "app"}
                  <th class="text-right py-2 font-medium">{t("usage_thRuns")}</th>
                {/if}
                <th class="text-right py-2 font-medium">{t("usage_thInTokens")}</th>
                <th class="text-right py-2 font-medium">{t("usage_thOutTokens")}</th>
                <th class="text-right py-2 font-medium">{t("usage_thCacheRead")}</th>
                <th class="text-right py-2 font-medium">{t("usage_thCacheWrite")}</th>
                <th class="text-right py-2 font-medium">{t("usage_thCost")}</th>
                <th class="text-right py-2 font-medium w-24">%</th>
              </tr>
            </thead>
            <tbody>
              {#each data.byModel as modelRow}
                <tr class="border-b border-border/50 hover:bg-muted/30">
                  <td class="py-2 font-mono text-xs truncate max-w-[180px]" title={modelRow.model}>
                    {modelRow.model}
                  </td>
                  {#if scope === "app"}
                    <td class="py-2 text-right tabular-nums">{modelRow.runs}</td>
                  {/if}
                  <td class="py-2 text-right tabular-nums font-mono text-xs">
                    {formatTokenCount(modelRow.inputTokens)}
                  </td>
                  <td class="py-2 text-right tabular-nums font-mono text-xs">
                    {formatTokenCount(modelRow.outputTokens)}
                  </td>
                  <td class="py-2 text-right tabular-nums font-mono text-xs text-muted-foreground">
                    {formatTokenCount(modelRow.cacheReadTokens)}
                  </td>
                  <td class="py-2 text-right tabular-nums font-mono text-xs text-muted-foreground">
                    {formatTokenCount(modelRow.cacheWriteTokens)}
                  </td>
                  <td class="py-2 text-right tabular-nums font-mono text-xs">
                    {formatCost(modelRow.costUsd)}
                  </td>
                  <td class="py-2 text-right">
                    <div class="flex items-center justify-end gap-2">
                      <div class="w-12 h-1.5 bg-muted rounded-full overflow-hidden">
                        <div
                          class="h-full bg-primary rounded-full"
                          style="width: {Math.min(modelRow.pct, 100)}%"
                        ></div>
                      </div>
                      <span class="text-xs tabular-nums text-muted-foreground w-8 text-right">
                        {modelRow.pct.toFixed(0)}%
                      </span>
                    </div>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {:else}
        <p class="text-sm text-muted-foreground">{t("usage_noModelData")}</p>
      {/if}
    </Card>

    <!-- Run History (App mode only) -->
    {#if scope === "app"}
      <Card class="p-6 space-y-4">
        <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
          {t("usage_runHistory")}
        </h2>
        {#if sortedRuns.length > 0}
          <div class="overflow-x-auto">
            <table class="w-full text-sm">
              <thead>
                <tr class="text-xs text-muted-foreground border-b border-border">
                  <th
                    class="text-left py-2 font-medium cursor-pointer select-none hover:text-foreground"
                    onclick={() => toggleSort("date")}
                  >
                    {t("usage_thDate")}{sortIndicator("date")}
                  </th>
                  <th class="text-left py-2 font-medium">{t("usage_thName")}</th>
                  <th class="text-left py-2 font-medium">{t("usage_thModel")}</th>
                  <th
                    class="text-right py-2 font-medium cursor-pointer select-none hover:text-foreground"
                    onclick={() => toggleSort("tokens")}
                  >
                    {t("usage_thTokens")}{sortIndicator("tokens")}
                  </th>
                  <th
                    class="text-right py-2 font-medium cursor-pointer select-none hover:text-foreground"
                    onclick={() => toggleSort("cost")}
                  >
                    {t("usage_thCost")}{sortIndicator("cost")}
                  </th>
                  <th
                    class="text-right py-2 font-medium cursor-pointer select-none hover:text-foreground"
                    onclick={() => toggleSort("turns")}
                  >
                    {t("usage_thTurns")}{sortIndicator("turns")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {#each sortedRuns as run}
                  <tr
                    class="border-b border-border/50 hover:bg-muted/30 cursor-pointer"
                    onclick={() => goto(`/chat?run=${run.runId}`)}
                  >
                    <td class="py-2 text-xs text-muted-foreground whitespace-nowrap">
                      {formatDate(run.startedAt)}
                    </td>
                    <td class="py-2 truncate max-w-[200px]" title={run.name}>
                      {run.name}
                    </td>
                    <td
                      class="py-2 font-mono text-xs text-muted-foreground truncate max-w-[120px]"
                      title={run.model ?? run.agent}
                    >
                      {run.model ?? run.agent}
                    </td>
                    <td class="py-2 text-right tabular-nums font-mono text-xs">
                      {formatTokenCount(run.inputTokens + run.outputTokens)}
                    </td>
                    <td class="py-2 text-right tabular-nums font-mono text-xs">
                      {formatCost(run.totalCostUsd)}
                    </td>
                    <td class="py-2 text-right tabular-nums">
                      {run.numTurns}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else}
          <p class="text-sm text-muted-foreground">
            {t("usage_noUsageData")}
          </p>
        {/if}
      </Card>
    {/if}
  {/if}

  {:else if mainTab === "proxy"}
  <!-- Proxy tab content -->

  <!-- Health timeline (30-min slot grid) -->
  <Card class="p-6 space-y-3">
    <div class="flex items-center justify-between">
      <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
        {t("proxy_health")}
      </h2>
      <span class="text-[11px] text-muted-foreground">{t("proxy_healthHours", { hours: 24 })}</span>
    </div>
    {#if proxyHealth.length > 0}
      {@const CELL_W = 14}
      {@const CELL_H = 24}
      {@const GAP = 2}
      {@const STEP = CELL_W + GAP}
      <!-- Build a map from slot string -> { success, error } -->
      {@const slotMap = (() => {
        const m = new Map<string, { success: number; error: number }>();
        for (const h of proxyHealth) {
          const existing = m.get(h.date) ?? { success: 0, error: 0 };
          existing.success += h.successCount;
          existing.error += h.errorCount;
          m.set(h.date, existing);
        }
        return m;
      })()}
      {@const allValues = [...slotMap.values()].map(v => v.success + v.error)}
      {@const nonZero = allValues.filter(v => v > 0).sort((a, b) => a - b)}
      {@const thresholds = (() => {
        if (nonZero.length === 0) return [0, 0, 0] as [number, number, number];
        const p = (pct: number) => nonZero[Math.floor((pct / 100) * (nonZero.length - 1))];
        return [p(25), p(50), p(75)] as [number, number, number];
      })()}
      <!-- Generate all 48 slots (24h × 2 per hour), ordered oldest→newest -->
      {@const slots = (() => {
        const result: string[] = [];
        const now = new Date();
        for (let i = 47; i >= 0; i--) {
          const d = new Date(now.getTime() - i * 30 * 60 * 1000);
          const min = d.getUTCMinutes() < 30 ? 0 : 30;
          result.push(`${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, '0')}-${String(d.getUTCDate()).padStart(2, '0')}T${String(d.getUTCHours()).padStart(2, '0')}:${String(min).padStart(2, '0')}`);
        }
        return result;
      })()}
      <!-- Extract time labels for every 2-hour boundary -->
      {@const timeLabels = (() => {
        const labels: { label: string; idx: number }[] = [];
        slots.forEach((slot, idx) => {
          const time = slot.slice(11); // "HH:MM"
          if (time === '00:00' || time === '02:00' || time === '04:00' || time === '06:00' || time === '08:00' || time === '10:00' || time === '12:00' || time === '14:00' || time === '16:00' || time === '18:00' || time === '20:00' || time === '22:00') {
            labels.push({ label: time, idx });
          }
        });
        return labels;
      })()}

      <!-- Cells row -->
      <div class="overflow-x-auto">
        <div class="flex" style="gap: {GAP}px; min-width: max-content;">
          {#each slots as slot, i}
            {@const val = slotMap.get(slot) ?? { success: 0, error: 0 }}
            {@const total = val.success + val.error}
            {@const level = total <= 0 ? 0 : total <= thresholds[0] ? 1 : total <= thresholds[1] ? 2 : total <= thresholds[2] ? 3 : 4}
            {@const cellClass = level === 0 ? 'bg-muted/30'
              : level === 1 ? 'bg-emerald-500/20'
              : level === 2 ? 'bg-emerald-500/40'
              : level === 3 ? 'bg-emerald-500/65'
              : 'bg-emerald-500'}
            <div
              class="shrink-0 rounded-[2px] {cellClass}"
              style="width: {CELL_W}px; height: {CELL_H}px;"
              title="{slot}: {t('proxy_success')} {val.success}, {t('proxy_error')} {val.error}, {t('proxy_thTotalT')} {total}"
            ></div>
          {/each}
        </div>

        <!-- Time labels -->
        <div class="relative" style="height: 16px; margin-top: 2px;">
          {#each timeLabels as tl}
            <span class="absolute text-[9px] text-muted-foreground select-none whitespace-nowrap" style="left: {tl.idx * STEP}px; transform: translateX(-25%);">
              {tl.label}
            </span>
          {/each}
        </div>
      </div>

      <!-- Legend -->
      <div class="flex items-center gap-1 text-[10px] text-muted-foreground justify-end select-none">
        <span>{t("usage_heatmapLess")}</span>
        <div class="rounded-[2px] bg-muted/30" style="width: {CELL_W}px; height: {CELL_W}px;"></div>
        <div class="rounded-[2px] bg-emerald-500/20" style="width: {CELL_W}px; height: {CELL_W}px;"></div>
        <div class="rounded-[2px] bg-emerald-500/40" style="width: {CELL_W}px; height: {CELL_W}px;"></div>
        <div class="rounded-[2px] bg-emerald-500/65" style="width: {CELL_W}px; height: {CELL_W}px;"></div>
        <div class="rounded-[2px] bg-emerald-500" style="width: {CELL_W}px; height: {CELL_W}px;"></div>
        <span>{t("usage_heatmapMore")}</span>
      </div>
    {:else}
      <p class="text-sm text-muted-foreground py-8 text-center">{t("proxy_noHealthData")}</p>
    {/if}
  </Card>

  <!-- Request log -->
  <Card class="p-6 space-y-4">
    <div class="flex items-center justify-between">
      <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
        {t("proxy_requestLog")}
      </h2>
      <div class="flex gap-2">
        <select
          class="rounded-md border bg-background px-2 py-1 text-[11px]"
          onchange={(e) => { proxyFilterModel = (e.target as HTMLSelectElement).value || null; proxyPage = 0; loadProxyData(); }}
        >
          <option value="">{t("proxy_filterAll")} {t("proxy_thModel")}</option>
          {#each proxyFilterValues.models as m}
            <option value={m} selected={proxyFilterModel === m}>{m}</option>
          {/each}
        </select>
        <select
          class="rounded-md border bg-background px-2 py-1 text-[11px]"
          onchange={(e) => { proxyFilterProvider = (e.target as HTMLSelectElement).value || null; proxyPage = 0; loadProxyData(); }}
        >
          <option value="">{t("proxy_filterAll")} {t("proxy_thProvider")}</option>
          {#each proxyFilterValues.providers as p}
            <option value={p} selected={proxyFilterProvider === p}>{p}</option>
          {/each}
        </select>
      </div>
    </div>

    {#if proxyLogs.length > 0}
      <div class="overflow-x-auto -mx-2">
        <table class="w-full text-[11px] min-w-[800px]">
          <thead>
            <tr class="border-b text-left text-muted-foreground">
              <th class="pb-2 px-2 font-medium whitespace-nowrap">{t("proxy_thTime")}</th>
              <th class="pb-2 px-2 font-medium whitespace-nowrap">{t("proxy_thModel")}</th>
              <th class="pb-2 px-2 font-medium whitespace-nowrap">{t("proxy_thProvider")}</th>
              <th class="pb-2 px-2 font-medium whitespace-nowrap">{t("proxy_thResult")}</th>
              <th class="pb-2 px-2 font-medium text-right whitespace-nowrap">{t("proxy_thLatency")}</th>
              <th class="pb-2 px-2 font-medium text-right whitespace-nowrap">{t("proxy_thInputT")}</th>
              <th class="pb-2 px-2 font-medium text-right whitespace-nowrap">{t("proxy_thOutputT")}</th>
              <th class="pb-2 px-2 font-medium text-right whitespace-nowrap">{t("proxy_thThinkingT")}</th>
              <th class="pb-2 px-2 font-medium text-right whitespace-nowrap">{t("proxy_thCacheT")}</th>
              <th class="pb-2 px-2 font-medium text-right whitespace-nowrap">{t("proxy_thTotalT")}</th>
            </tr>
          </thead>
          <tbody>
            {#each proxyLogs as log}
              <tr class="border-b border-border/50 hover:bg-accent/30 transition-colors">
                <td class="py-2 px-2 whitespace-nowrap text-muted-foreground font-mono">{log.ts.slice(0, 16).replace("T", " ")}</td>
                <td class="py-2 px-2 font-mono max-w-[160px] truncate">{log.model}</td>
                <td class="py-2 px-2 text-muted-foreground max-w-[120px] truncate">{log.providerId}</td>
                <td class="py-2 px-2">
                  <span class="inline-flex items-center rounded-sm px-1.5 py-0.5 text-[10px] font-medium
                    {log.result === 'success' ? 'bg-emerald-500/10 text-emerald-600' : 'bg-red-500/10 text-red-600'}">
                    {log.statusCode}
                  </span>
                </td>
                <td class="py-2 px-2 text-right tabular-nums text-muted-foreground">{log.latencyMs}ms</td>
                <td class="py-2 px-2 text-right tabular-nums">{log.inputTokens ?? '—'}</td>
                <td class="py-2 px-2 text-right tabular-nums">{log.outputTokens ?? '—'}</td>
                <td class="py-2 px-2 text-right tabular-nums">{log.thinkingTokens ?? '—'}</td>
                <td class="py-2 px-2 text-right tabular-nums">
                  {#if log.cacheReadTokens || log.cacheCreationTokens}
                    {(log.cacheReadTokens ?? 0) + (log.cacheCreationTokens ?? 0)}
                  {:else}—{/if}
                </td>
                <td class="py-2 px-2 text-right tabular-nums font-medium">
                  {#if proxyTotalTokens(log) > 0}{proxyTotalTokens(log)}{:else}—{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>

      <!-- Pagination -->
      <div class="flex items-center justify-between pt-2">
        <button
          class="rounded-md border px-3 py-1 text-[11px] text-muted-foreground hover:bg-accent disabled:opacity-40 transition-colors"
          disabled={proxyPage === 0}
          onclick={() => { proxyPage--; loadProxyData(); }}
        >
          {t("proxy_prev")}
        </button>
        <span class="text-[11px] text-muted-foreground">
          {Math.min(proxyPage * PROXY_PAGE_SIZE + 1, proxyTotal)}–{Math.min((proxyPage + 1) * PROXY_PAGE_SIZE, proxyTotal)} / {proxyTotal}
        </span>
        <button
          class="rounded-md border px-2.5 py-1 text-[11px] text-muted-foreground hover:bg-accent disabled:opacity-40"
          disabled={(proxyPage + 1) * PROXY_PAGE_SIZE >= proxyTotal}
          onclick={() => { proxyPage++; loadProxyData(); }}
        >
          {t("proxy_next")}
        </button>
      </div>
    {:else}
      <p class="text-sm text-muted-foreground py-8 text-center">{t("proxy_noLogs")}</p>
    {/if}
  </Card>

  {/if}
</div>
