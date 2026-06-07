<script>
  import { fly } from "svelte/transition";
  import { quartOut } from "svelte/easing";
  import { isAuthenticated, isAuthLoading } from "./stores/auth";
  import { initConversation } from "./stores/conversation";
  import Login from "./pages/Login.svelte";
  import Dashboard from "./pages/Dashboard.svelte";
  import SimDashboard from "./pages/SimDashboard.svelte";
  import CallLogPage from "./pages/CallLogPage.svelte";
  import SimCardsPage from "./pages/SimCardsPage.svelte";

  /** @type {'sim' | 'messages' | 'calllog' | 'simcards'} */
  let currentPage = $state('sim');
  let filterSimId = $state(null);

  function goToSim() {
    filterSimId = null;
    currentPage = 'sim';
  }

  function goToMessages(simId) {
    filterSimId = simId;
    currentPage = 'messages';
  }

  function goToCallLog(simId) {
    filterSimId = simId;
    currentPage = 'calllog';
  }

  function goToSimCards(simId) {
    filterSimId = simId;
    currentPage = 'simcards';
  }

  $effect(() => {
    if ($isAuthenticated) {
      initConversation();
    }
  });
</script>

<div class="app-container">
  {#if !$isAuthLoading}
    <div class="h-dvh w-screen overflow-hidden">
      {#if $isAuthenticated}

        {#if currentPage === 'sim'}
          <div in:fly={{ x: -40, duration: 350, easing: quartOut }} out:fly={{ x: -40, duration: 250, easing: quartOut }}>
            <SimDashboard
              onNavigate={goToMessages}
              onNavigateCall={goToCallLog}
              onNavigateSim={goToSimCards}
            />
          </div>

        {:else if currentPage === 'messages'}
          <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
            <Dashboard onNavigate={goToSim} initialSimId={filterSimId} />
          </div>

        {:else if currentPage === 'calllog'}
          <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
            <CallLogPage onBack={goToSim} filterSimId={filterSimId} />
          </div>

        {:else if currentPage === 'simcards'}
          <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
            <SimCardsPage onBack={goToSim} filterSimId={filterSimId} />
          </div>
        {/if}

      {:else}
        <div
          in:fly={{ x: -50, duration: 500, easing: quartOut }}
          out:fly={{ x: 50, duration: 300, easing: quartOut }}
        >
          <Login />
        </div>
      {/if}
    </div>
  {/if}
</div>
