<script lang="ts">
  let {
    value = $bindable(""),
    placeholder = "",
    type = "text",
    disabled = false,
    class: className = "",
    oninput,
    onblur,
    onpaste,
    autocorrect = "off",
    autocapitalize = "off",
  }: {
    value?: string;
    placeholder?: string;
    type?: string;
    disabled?: boolean;
    class?: string;
    oninput?: (e: Event) => void;
    onblur?: (e: FocusEvent) => void;
    onpaste?: (e: ClipboardEvent) => void;
    autocorrect?: "on" | "off";
    autocapitalize?: "none" | "sentences" | "words" | "characters" | "off" | "on";
  } = $props();
</script>

<input
  {type}
  {placeholder}
  {disabled}
  bind:value
  {oninput}
  {onblur}
  {onpaste}
  {autocorrect}
  {autocapitalize}
  class="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 {className}"
/>
