import { useCallback, useEffect, useState } from "react";
import { api, type AppInfo } from "./api";
import { NavContext, isNavLocked, setCutoff, useKey, type Route } from "./nav";
import { Backdrop, TopNav } from "./ui";
import { ToastProvider } from "./toast";
import { Home } from "./screens/Home";
import { NewDeckModal } from "./screens/NewDeck";
import { Deck } from "./screens/Deck";
import { AddCard } from "./screens/AddCard";
import { Study, Summary } from "./screens/Study";
import { Done } from "./screens/Done";
import { Cards } from "./screens/Cards";
import { DeckSettings } from "./screens/DeckSettings";
import { Import, ImportResultScreen } from "./screens/Import";
import { Importance } from "./screens/Importance";
import { Settings } from "./screens/Settings";
import { Recover } from "./screens/Recover";

export default function App() {
  const [route, setRoute] = useState<Route>({ name: "home" });
  const [ready, setReady] = useState(false);
  // 학습 데이터를 열지 못했으면 (S1-6) 화면 대신 복구 안내를 띄운다
  const [broken, setBroken] = useState<AppInfo | null>(null);
  const load = useCallback(() => {
    api.appInfo().then(
      (i) => {
        setCutoff(i.cutoff_hour);
        setBroken(i.startup_error ? i : null);
      },
      () => {},
    ).finally(() => setReady(true));
  }, []);
  useEffect(load, [load]);
  const nav = useCallback((r: Route) => {
    if (isNavLocked()) return;
    setRoute(r);
    window.scrollTo(0, 0);
  }, []);

  // 학습 중에는 상단 내비게이션과 배경 장식을 끈다 (브리프 3.7)
  const focused = route.name === "study";

  // 전역 단축키 N · I (E1 기본 단축키). 학습 흐름과 작성 중인 카드 추가 화면에서는 끈다
  const globalKeys = !["study", "summary", "addCard"].includes(route.name);
  useKey({ action: "newDeck" }, useCallback(() => nav({ name: "newDeck" }), [nav]), globalKeys);
  useKey({ action: "import" }, useCallback(() => nav({ name: "import", pick: true }), [nav]), globalKeys);

  return (
    <ToastProvider>
      <NavContext.Provider value={nav}>
        <div className="app">
          {!focused && <Backdrop />}
          {!focused && (
            <div className="app__nav">
              <TopNav
                onHome={() => nav({ name: "home" })}
                onImport={() => nav({ name: "import" })}
                onNewDeck={() => nav({ name: "newDeck" })}
                onSettings={() => nav({ name: "settings" })}
              />
            </div>
          )}
          {ready && (broken ? <Recover info={broken} onRecovered={() => (load(), nav({ name: "home" }))} /> : <Screen route={route} />)}
        </div>
      </NavContext.Provider>
    </ToastProvider>
  );
}

function Screen({ route }: { route: Route }) {
  switch (route.name) {
    case "home":
      return <Home />;
    case "newDeck":
      return (
        <>
          <Home />
          <NewDeckModal />
        </>
      );
    case "deck":
      return <Deck key={route.deckId} deckId={route.deckId} />;
    case "addCard":
      return <AddCard key={route.deckId} deckId={route.deckId} />;
    case "cards":
      return (
        <Cards
          key={`${route.deckId}-${route.buriedOnly}-${route.cardIds}-${route.weakOnly}`}
          deckId={route.deckId}
          buriedOnly={route.buriedOnly}
          cardIds={route.cardIds}
          weakOnly={route.weakOnly}
        />
      );
    case "import":
      return <Import key="anki" tab="anki" pick={route.pick} />;
    case "importJson":
      return <Import key={`json-${route.deckId}`} tab="json" deckId={route.deckId ?? null} />;
    case "importResult":
      return <ImportResultScreen result={route.result} />;
    case "importance":
      return <Importance key={route.deckId} deckId={route.deckId} next={route.next} />;
    case "settings":
      return <Settings focus={route.focus} />;
    case "deckSettings":
      return <DeckSettings key={route.deckId} deckId={route.deckId} />;
    case "summary":
      return <Summary deckIds={route.deckIds} />;
    case "study":
      return <Study key={route.sessionId ?? "new"} {...route} />;
    case "done":
      return <Done {...route} />;
    default:
      return <Home />;
  }
}
