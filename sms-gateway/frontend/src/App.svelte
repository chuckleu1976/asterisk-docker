<script>
  import { fly } from "svelte/transition";
  import { quartOut } from "svelte/easing";
  import { initConversation, disconnectConversation } from "./stores/conversation";
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
  import TelegramPage from "./pages/TelegramPage.svelte";
  import Login from "./pages/Login.svelte";
  import Account from "./pages/Account.svelte";
  import { user, isAuthenticated, isAuthLoading } from "./stores/auth.js";

  /** @type {'sim' | 'messages' | 'calllog' | 'simcards' | 'platform' | 'phonenumber' | 'platform-stats' | 'money' | 'mms' | 'esim' | 'telegram' | 'account'} */
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

  function goToTelegram() {
    currentPage = 'telegram';
  }

  function backFromEsim() {
    currentPage = $user?.is_admin ? 'phonenumber' : 'account';
  }

  $effect(() => {
    if ($user) currentPage = $user.is_admin ? 'sim' : 'account';
  });

  $effect(() => {
    if (!$isAuthenticated || !$user?.is_admin) return;
    initConversation();
    connectCallSSE();

    return () => {
      disconnectCallSSE();
      disconnectConversation();
    };
  });
</script>

<div class="app-container">
  <div class="h-dvh w-screen overflow-hidden">

    {#if $isAuthLoading}
      <div class="h-dvh flex items-center justify-center text-gray-500" role="status">Loading...</div>
    {:else if !$isAuthenticated}
      <Login />
    {:else if currentPage === 'account' || (!$user?.is_admin && currentPage !== 'esim')}
      <Account onManage={goToEsim} onDashboard={goToSim} />
    {:else if currentPage === 'sim'}
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
          onNavigateTelegram={goToTelegram}
          onNavigateAccount={() => currentPage = 'account'}
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

    {:else if currentPage === 'telegram'}
      <div in:fly={{ x: 40, duration: 350, easing: quartOut }} out:fly={{ x: 40, duration: 250, easing: quartOut }}>
        <TelegramPage onBack={goToSim} />
      </div>
    {/if}

  </div>
</div>
