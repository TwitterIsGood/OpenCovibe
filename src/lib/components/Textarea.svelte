<script lang="ts">
  let {
    value = $bindable(""),
    placeholder = "",
    rows = 3,
    disabled = false,
    class: className = "",
    onkeydown,
    autocorrect = "off",
    autocapitalize = "off",
  }: {
    value?: string;
    placeholder?: string;
    rows?: number;
    disabled?: boolean;
    class?: string;
    onkeydown?: (e: KeyboardEvent) => void;
    autocorrect?: "on" | "off";
    autocapitalize?: "none" | "sentences" | "words" | "characters" | "off" | "on";
  } = $props();
</script>

<textarea
  {placeholder}
  {rows}
  {disabled}
  bind:value
  {onkeydown}
  {autocorrect}
  {autocapitalize}
  class="flex min-h-[60px] w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 resize-none {className}"
></textarea>
