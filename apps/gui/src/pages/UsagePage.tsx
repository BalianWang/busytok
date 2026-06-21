import { useState, useRef, useEffect, useCallback } from "react";
import { ActivityPage } from "./ActivityPage";
import { ProjectsPage } from "./ProjectsPage";
import { ModelsPage } from "./ModelsPage";
import { SessionsPage } from "./SessionsPage";

type UsageTab = "activity" | "projects" | "models" | "sessions";

interface TabDef {
  id: UsageTab;
  label: string;
}

const TABS: TabDef[] = [
  { id: "activity", label: "Activity" },
  { id: "projects", label: "Projects" },
  { id: "models", label: "Models" },
  { id: "sessions", label: "Sessions" },
];

export function UsagePage() {
  const [activeTab, setActiveTab] = useState<UsageTab>("activity");
  const tabRefs = useRef<Map<string, HTMLButtonElement>>(new Map());
  const [indicatorStyle, setIndicatorStyle] = useState({ left: 0, width: 0 });

  const updateIndicator = useCallback(() => {
    const el = tabRefs.current.get(activeTab);
    if (!el) return;
    const parent = el.parentElement;
    if (!parent) return;
    setIndicatorStyle({
      left: el.offsetLeft,
      width: el.offsetWidth,
    });
  }, [activeTab]);

  useEffect(() => {
    updateIndicator();
    const onResize = () => updateIndicator();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [updateIndicator]);

  const handleKeyDown = (e: React.KeyboardEvent, currentIndex: number) => {
    let nextIndex: number | null = null;
    if (e.key === "ArrowRight") nextIndex = (currentIndex + 1) % TABS.length;
    if (e.key === "ArrowLeft") nextIndex = (currentIndex - 1 + TABS.length) % TABS.length;
    if (nextIndex !== null) {
      e.preventDefault();
      setActiveTab(TABS[nextIndex].id);
      tabRefs.current.get(TABS[nextIndex].id)?.focus();
    }
  };

  const content: Record<UsageTab, React.ReactNode> = {
    activity: <ActivityPage />,
    projects: <ProjectsPage />,
    models: <ModelsPage />,
    sessions: <SessionsPage />,
  };

  return (
    <div className="usage-page">
      <div className="usage-tabs" role="tablist">
        {TABS.map((tab, index) => (
          <button
            key={tab.id}
            ref={(el) => {
              if (el) tabRefs.current.set(tab.id, el);
            }}
            role="tab"
            aria-selected={tab.id === activeTab}
            tabIndex={tab.id === activeTab ? 0 : -1}
            className={`usage-tabs__tab${tab.id === activeTab ? " is-active" : ""}`}
            onClick={() => setActiveTab(tab.id)}
            onKeyDown={(e) => handleKeyDown(e, index)}
          >
            {tab.label}
          </button>
        ))}
        <div
          className="usage-tabs__indicator"
          style={{ transform: `translateX(${indicatorStyle.left}px)`, width: indicatorStyle.width }}
        />
      </div>
      <div className="usage-content">
        {content[activeTab]}
      </div>
    </div>
  );
}
