import { Activity, BarChart3, Command, Settings, type LucideIcon } from "lucide-react";
import type { DesktopPage } from "../AppShell";

interface SidebarProps {
  currentPage: DesktopPage;
  onNavigate: (page: DesktopPage) => void;
}

interface SidebarGroup {
  label?: string;
  items: Array<{ id: DesktopPage; label: string; icon: LucideIcon }>;
}

const GROUPS: SidebarGroup[] = [
  {
    label: "Monitoring",
    items: [
      { id: "overview", label: "Overview", icon: BarChart3 },
      { id: "usage", label: "Usage", icon: Activity },
    ],
  },
  {
    label: "Tools",
    items: [{ id: "prompt_palette", label: "Prompt Palette", icon: Command }],
  },
  {
    label: "System",
    items: [{ id: "settings", label: "Settings", icon: Settings }],
  },
];

export function Sidebar({ currentPage, onNavigate }: SidebarProps) {
  return (
    <aside className="desktop-sidebar">
      <nav aria-label="Desktop sections" className="desktop-sidebar__nav">
        {GROUPS.map((group, groupIndex) => (
          <section
            className="desktop-sidebar__group"
            key={group.label ?? `group-${groupIndex}`}
          >
            {group.label ? (
              <p className="desktop-sidebar__group-label">{group.label}</p>
            ) : null}
            {group.items.map((item) => {
              const Icon = item.icon;
              const active = item.id === currentPage;
              return (
                <button
                  key={item.id}
                  type="button"
                  className={`desktop-sidebar__item${active ? " is-active" : ""}`}
                  aria-current={active ? "page" : undefined}
                  onClick={() => onNavigate(item.id)}
                >
                  <Icon aria-hidden="true" size={17} strokeWidth={1.8} />
                  <span>{item.label}</span>
                </button>
              );
            })}
          </section>
        ))}
      </nav>
    </aside>
  );
}
