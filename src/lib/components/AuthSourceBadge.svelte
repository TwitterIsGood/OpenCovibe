<script lang="ts">
  import { goto } from "$app/navigation";
  import type { AuthOverview } from "$lib/types";
  import { t } from "$lib/i18n/index.svelte";
  import { onMount } from "svelte";

  let {
    authOverview = null,
    authSourceLabel = "",
    authSourceCategory = "unknown",
    apiKeySource = "",
    hasRun = false,
    authMode = "",
    onAuthModeChange,
    variant = "default",
  }: {
    authOverview?: AuthOverview | null;
    authSourceLabel?: string;
    authSourceCategory?: string;
    apiKeySource?: string;
    hasRun?: boolean;
    authMode?: string;
    onAuthModeChange?: (mode: string) => void;
    variant?: "default" | "hero";
  } = $props();

  let dropdownOpen = $state(false);
  let wrapperEl: HTMLDivElement | undefined = $state();
  let buttonEl: HTMLButtonElement | undefined = $state();
  let dropdownStyle = $state("");

  // ── Badge colors based on auth source category ──
  const BADGE_COLORS: Record<string, string> = {
    login: "bg-emerald-500/15 text-emerald-500",
    env_key: "bg-blue-500/15 text-blue-400",
    none: "bg-amber-500/15 text-amber-500",
    other: "bg-foreground/10 text-foreground/60",
  };

  let badgeColor = $derived(BADGE_COLORS[authSourceCategory] ?? "");

  // ── Pre-session display ──
  let preSessionLabel = $derived.by(() => {
    if (!authOverview) return "";
    if (authOverview.auth_mode === "cli") return t("auth_cliAuth");
    return t("auth_appApiKey");
  });

  let preSessionDotColor = $derived.by(() => {
    if (!authOverview) return "bg-muted-foreground/30";
    if (authOverview.auth_mode === "cli") {
      return authOverview.cli_login_available || authOverview.cli_has_api_key
        ? "bg-emerald-500"
        : "bg-amber-500";
    }
    return authOverview.app_has_credentials ? "bg-emerald-500" : "bg-amber-500";
  });

  // ── Loading label (shown before authOverview loads) ──
  let loadingLabel = $derived.by(() => {
    if (authMode === "cli") return t("auth_cliAuth");
    if (authMode === "api") return t("auth_appApiKey");
    return "";
  });

  function toggleDropdown() {
    if (hasRun) return;
    dropdownOpen = !dropdownOpen;
    if (dropdownOpen && buttonEl) updateDropdownPosition();
  }

  function updateDropdownPosition() {
    if (!buttonEl) return;
    const rect = buttonEl.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom;
    if (spaceBelow < 300) {
      dropdownStyle = `position:fixed; bottom:${window.innerHeight - rect.top + 4}px; left:${rect.left}px; z-index:50;`;
    } else {
      dropdownStyle = `position:fixed; top:${rect.bottom + 4}px; left:${rect.left}px; z-index:50;`;
    }
  }

  function selectMode(mode: string) {
    dropdownOpen = false;
    onAuthModeChange?.(mode);
  }

  onMount(() => {
    function onDocClick(e: MouseEvent) {
      if (dropdownOpen && wrapperEl && !wrapperEl.contains(e.target as Node)) {
        dropdownOpen = false;
      }
    }
    function onDocKeydown(e: KeyboardEvent) {
      if (dropdownOpen && e.key === "Escape") {
        dropdownOpen = false;
      }
    }
    document.addEventListener("mousedown", onDocClick, true);
    document.addEventListener("keydown", onDocKeydown);
    return () => {
      document.removeEventListener("mousedown", onDocClick, true);
      document.removeEventListener("keydown", onDocKeydown);
    };
  });
</script>

{#if hasRun && authSourceLabel}
  <!-- Phase B: During session — read-only badge showing CLI-reported source -->
  <span
    class="shrink-0 rounded-md px-2 py-0.5 text-[11px] font-medium {badgeColor}"
    title={t("statusbar_authTitle", { source: apiKeySource })}
  >
    {authSourceLabel}
  </span>
{:else if !hasRun && authOverview}
  <!-- Phase A: Before session — clickable badge with dropdown -->
  <div bind:this={wrapperEl} class="inline-flex items-center">
    <button
      bind:this={buttonEl}
      class="flex items-center gap-1.5 rounded-md transition-colors cursor-pointer
        {variant === 'hero'
        ? 'px-2.5 py-1 text-xs text-muted-foreground hover:text-foreground'
        : 'border px-2 py-1 text-xs font-medium hover:bg-accent'}"
      onclick={toggleDropdown}
      title={t("auth_sourceLabel")}
    >
      <span class="inline-block h-1.5 w-1.5 rounded-full {preSessionDotColor}"></span>
      {preSessionLabel}
      <svg
        class="h-2.5 w-2.5 text-muted-foreground"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
      >
    </button>

    {#if dropdownOpen}
      <div
        class="w-64 rounded-md border bg-background shadow-lg animate-fade-in"
        style={dropdownStyle}
      >
        <div class="p-2 space-y-1">
          <p
            class="px-2 pt-1 pb-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/60"
          >
            {t("settings_auth_modeLabel")}
          </p>

          <!-- CLI Auth option -->
          <button
            class="flex w-full items-start gap-2.5 rounded-sm px-2.5 py-2 text-sm hover:bg-accent transition-colors
              {authOverview.auth_mode === 'cli' ? 'bg-accent' : ''}"
            onclick={() => selectMode("cli")}
          >
            <span class="mt-0.5 inline-block h-3.5 w-3.5 shrink-0">
              {#if authOverview.auth_mode === "cli"}
                <svg
                  class="h-3.5 w-3.5 text-primary"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                >
                  <circle cx="12" cy="12" r="10" /><circle
                    cx="12"
                    cy="12"
                    r="4"
                    fill="currentColor"
                  />
                </svg>
              {:else}
                <svg
                  class="h-3.5 w-3.5 text-muted-foreground/50"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                >
                  <circle cx="12" cy="12" r="10" />
                </svg>
              {/if}
            </span>
            <div class="flex-1 text-left">
              <p class="font-medium text-xs">{t("auth_cliAuth")}</p>
              {#if authOverview.cli_login_available}
                <p class="text-[10px] text-emerald-500">
                  <span class="inline-block h-1 w-1 rounded-full bg-emerald-500 mr-0.5 align-middle"
                  ></span>
                  {t("auth_loggedIn")}{authOverview.cli_login_account
                    ? `: ${authOverview.cli_login_account}`
                    : ""}
                </p>
              {:else}
                <p class="text-[10px] text-muted-foreground">
                  <span
                    class="inline-block h-1 w-1 rounded-full bg-muted-foreground/40 mr-0.5 align-middle"
                  ></span>
                  {t("auth_notLoggedIn")}
                </p>
              {/if}
              {#if authOverview.cli_has_api_key}
                <p class="text-[10px] text-emerald-500">
                  <span class="inline-block h-1 w-1 rounded-full bg-emerald-500 mr-0.5 align-middle"
                  ></span>
                  {t("auth_cliKeyHint", { hint: authOverview.cli_api_key_hint ?? "" })}
                </p>
              {/if}
              {#if authOverview.cli_login_available && authOverview.cli_has_api_key}
                <p class="text-[10px] text-muted-foreground/70 italic mt-0.5">
                  {t("auth_cliPriorityHint")}
                </p>
              {/if}
            </div>
          </button>

          <!-- App API Key option -->
          <button
            class="flex w-full items-start gap-2.5 rounded-sm px-2.5 py-2 text-sm hover:bg-accent transition-colors
              {authOverview.auth_mode === 'api' ? 'bg-accent' : ''}"
            onclick={() => selectMode("api")}
          >
            <span class="mt-0.5 inline-block h-3.5 w-3.5 shrink-0">
              {#if authOverview.auth_mode === "api"}
                <svg
                  class="h-3.5 w-3.5 text-primary"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                >
                  <circle cx="12" cy="12" r="10" /><circle
                    cx="12"
                    cy="12"
                    r="4"
                    fill="currentColor"
                  />
                </svg>
              {:else}
                <svg
                  class="h-3.5 w-3.5 text-muted-foreground/50"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                >
                  <circle cx="12" cy="12" r="10" />
                </svg>
              {/if}
            </span>
            <div class="flex-1 text-left">
              <p class="font-medium text-xs">{t("auth_appApiKey")}</p>
              {#if authOverview.app_has_credentials}
                <p class="text-[10px] text-emerald-500">
                  <span
                    class="inline-block h-1 w-1 rounded-full bg-emerald-500 mr-0.5 align-middle"
                  ></span>
                  {t("auth_loggedIn")}
                </p>
              {:else}
                <p class="text-[10px] text-amber-500">
                  <span class="inline-block h-1 w-1 rounded-full bg-amber-500 mr-0.5 align-middle"
                  ></span>
                  {t("prompt_noPlatformKey")}
                </p>
              {/if}
            </div>
          </button>

          <!-- Configure link -->
          <button
            class="flex w-full items-center gap-1.5 rounded-sm px-2.5 py-1.5 text-xs text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
            onclick={() => {
              dropdownOpen = false;
              goto("/settings");
            }}
          >
            <svg
              class="h-3 w-3"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path
                d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"
              />
              <circle cx="12" cy="12" r="3" />
            </svg>
            {t("auth_configureInSettings")}
          </button>
        </div>
      </div>
    {/if}
  </div>
{:else if !hasRun && loadingLabel}
  <!-- Phase A (loading): authOverview not yet loaded — show static badge from settings -->
  <span
    class="inline-flex items-center gap-1.5 rounded-md border border-transparent px-2 py-1 text-xs font-medium text-muted-foreground/70"
  >
    <span class="inline-block h-1.5 w-1.5 rounded-full bg-muted-foreground/30"></span>
    {loadingLabel}
  </span>
{/if}
