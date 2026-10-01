<script>
  import { fly } from "svelte/transition";
  import { quartOut } from "svelte/easing";
  import { initConversation } from "./stores/conversation";
  import { connectCallSSE, disconnectCallSSE } from "./stores/calls.js";
  import Dashboard from "./pages/Dashboard.svelte";
  import SimDashboard from "./pages/SimDashboard.svelte";
  import CallLogPage from "./pages/CallLogPage.svelte";
  import SimCardsPage from "./pages/SimCardsPage.svelte";
  import PlatformConnectPage from "./pages/PlatformConnectPage.svelte";
  import PhoneNumberPage from "./pages/PhoneNumberPage.svelte";
  import PlatformStatisticsPage from "./pages/PlatformStatisticsPage.svelte";
  import MoneyPage from "./pages/MoneyPage.svelte";
  import MmsPage from "./pages/MmsPage.svelte";
  import EsimPage from "./pages/EsimPage.svelte";

  /** @type {'sim' | 'messages' | 'calllog' | 'simcards' | 'platform' | 'phonenumber' | 'platform-stats' | 'money' | 'mms' | 'esim'} */
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

  function goToPlatform() {
    currentPage = 'platform';
  }

  function goToPlatformStats() {
    currentPage = 'platform-stats';
  }

  function goToMoney() {
    currentPage = 'money';
  }

  function goToMms(simId) {
    filterSimId = simId;
    currentPage = 'mms';
  }

  function goToPhoneNumber() {
    currentPage = 'phonenumber';
  }

  function goToEsim() {
    currentPage = 'esim';
  }

  function backFromEsim() {
    currentPage = 'phonenumber';
  }

  $effect(() => {
    initConversation();
    connectCallSSE();

    return () => {
      disconnectCallSSE();
    };
  });
</script>

<div class="app-container">
  <div class="h-dvh w-screen overflow-hidden">

    {#if currentPage === 'sim'}
      <div in:fly={{ x: -40, duration: 350, easing: quartOut }} out:fly={{ x: -40, duration: 250, easing: quartOut }}>
        <SimDashboard
          onNavigate={goToMessages}
          onNavigateCall={goToCallLog}
          onNavigateSim={goToSimCards}
          onNavigatePlatform={goToPlatform}
          onNavigatePhoneNumber={goToPhoneNumber}
          onNavigatePlatformStats={goToPlatformStats}
          onNavigateMoney={goToMoney}
          onNavigateMms={goToMms}
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

    {:else if currentPage === 'platform'}
      <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
        <PlatformConnectPage onBack={goToSim} />
      </div>

    {:else if currentPage === 'platform-stats'}
      <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
        <PlatformStatisticsPage onBack={goToSim} />
      </div>

    {:else if currentPage === 'money'}
      <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
        <MoneyPage onBack={goToSim} />
      </div>

    {:else if currentPage === 'mms'}
      <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
        <MmsPage onBack={goToSim} initialSimId={filterSimId} />
      </div>

    {:else if currentPage === 'phonenumber'}
      <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
        <PhoneNumberPage onBack={goToSim} onNavigateEsim={goToEsim} />
      </div>

    {:else if currentPage === 'esim'}
      <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
        <EsimPage onBack={backFromEsim} />
      </div>
    {/if}

  </div>
</div>
