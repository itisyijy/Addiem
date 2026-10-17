import { useCallback, useEffect, useState } from "react";
import { api, type AppInfo } from "./api";
import { NavContext, isNavLocked, leaveBlocked, setCutoff, useKey, type Route } from "./nav";
import { Backdrop, TopNav } from "./ui";
import { ToastProvider } from "./toast";
import { SyncButton, SyncHost } from "./sync";
import { Home } from "./screens/Home";
import { NewDeckModal } from "./screens/NewDeck";
import { Deck } from "./screens/Deck";
import { AddCard } from "./screens/AddCard";
import { Study, Summary } from "./screens/Study";
import { Done } from "./screens/Done";
import { MixDone, PreExam } from "./screens/PreExam";
import { Cards } from "./screens/Cards";
import { DeckSettings } from "./screens/DeckSettings";
import { Import, ImportResultScreen } from "./screens/Import";
import { Importance } from "./screens/Importance";
import { Settings } from "./screens/Settings";
import { Recover } from "./screens/Recover";
import { Quality } from "./screens/Quality";
import { Sources } from "./screens/Sources";
import { Drafts } from "./screens/Drafts";
import { Plan, PlanConflictScreen } from "./screens/Plan";
import { NoteTypes } from "./screens/NoteTypes";
import { NoteTypeEdit } from "./screens/NoteTypeEdit";

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
  const nav = useCallback((r: Route, force = false) => {
    if (isNavLocked() || (!force && leaveBlocked(r))) return;
    setRoute(r);
    window.scrollTo(0, 0);
  }, []);

  // 학습 중에는 상단 내비게이션과 배경 장식을 끈다 (브리프 3.7)
  const focused = route.name === "study";
  // NT2 노트 유형 편집기는 M2 처럼 제 상단 바를 쓰는 전체 화면
  const bare = focused || route.name === "noteTypeEdit";

  // 전역 단축키 N · I (E1 기본 단축키). 학습 흐름과 작성 중인 카드 추가 화면에서는 끈다
  const globalKeys = !["study", "summary", "addCard", "noteTypeEdit"].includes(route.name);
  useKey({ action: "newDeck" }, useCallback(() => nav({ name: "newDeck" }), [nav]), globalKeys);
  useKey({ action: "import" }, useCallback(() => nav({ name: "import", pick: true }), [nav]), globalKeys);

  return (
    <ToastProvider>
      <NavContext.Provider value={nav}>
        <div className="app">
          {!bare && <Backdrop />}
          {!bare && (
            <div className="app__nav">
              <TopNav
                onHome={() => nav({ name: "home" })}
                onImport={() => nav({ name: "import" })}
                onNewDeck={() => nav({ name: "newDeck" })}
                onSettings={() => nav({ name: "settings" })}
                sync={<SyncButton />}
              />
            </div>
          )}
          {ready && (broken ? <Recover info={broken} onRecovered={() => (load(), nav({ name: "home" }))} /> : <Screen route={route} />)}
          {/* K Drive 동기화 창들. 학습 화면에서는 동기화를 멈추고, 나오면(학습을 마치면) 올린다 */}
          <SyncHost studying={focused} />
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
          edit={route.edit}
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
    case "noteTypes":
      return <NoteTypes back={route.back} />;
    case "noteTypeEdit":
      return <NoteTypeEdit key={route.id} id={route.id} ord={route.ord} note={route.note} back={route.back} />;
    case "deckSettings":
      return <DeckSettings key={route.deckId} deckId={route.deckId} />;
    case "quality":
      return <Quality key={route.deckId} deckId={route.deckId} />;
    case "sources": // I1 자료
      return <Sources key={`${route.deckId}-${route.segmentId}-${route.gaps}`} {...route} />;
    case "drafts": // R1-c 초안 검토
      return <Drafts {...route} />;
    case "plan":
      return <Plan key={route.deckId ?? "all"} deckId={route.deckId} />;
    case "planConflict":
      return <PlanConflictScreen back={route.back} />;
    case "summary":
      return route.cram ? <PreExam key="cram" deckIds={route.deckIds} /> : <Summary key="d1" deckIds={route.deckIds} />;
    case "study":
      return <Study key={route.sessionId ?? "new"} {...route} />;
    case "done":
      return <Done {...route} />;
    case "mixDone":
      return <MixDone {...route} />;
    default:
      return <Home />;
  }
}
