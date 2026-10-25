<script context="module" lang="ts">
  export function pageTitle(): string {
    return "Albums";
  }
</script>

<h1>{pageTitle()}</h1>
<ul>
  {#each albums as album}
    <li>{album}</li>
  {/each}
</ul>
<button on:click={load}>reload</button>

<script lang="ts">
  import { onMount } from "svelte";
  import { getAlbums } from "../../lib/api";

  let albums: unknown[] = [];
  let users: unknown[] = [];

  async function load(): Promise<void> {
    albums = (await getAlbums()) as unknown[];
    const res = await fetch("/api/users");
    users = await res.json();
  }

  onMount(load);
</script>
