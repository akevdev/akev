import { AppWindow, ArrowLeftRight, PanelBottom, PanelRight, PanelTop, Plug, Rocket, Send, Sparkles, Workflow } from 'lucide-react';
import { navigate } from '../../../app/router';
import { useResolvedTheme } from '../../../app/theme';
import { Button, SegmentedControl, Select, Switch } from '../../../components/ui';
import { customTitleBarAvailable } from '../../../lib/windowChrome';
import { openFirstRun } from '../../traffic/first-run/firstRun';
import { DENSITIES, UI_SCALES, useUiStore, type Density, type DetailPosition, type StartPage, type TitleBarPref, type UiScale } from '../../../state/ui';
import { SettingRow, SettingsCard } from '../SettingRow';
import { ThemePicker } from '../ThemePicker';
import styles from './sections.module.css';

const START_PAGES: { value: StartPage; label: string; icon: typeof Plug }[] = [
  { value: 'traffic', label: 'Traffic', icon: ArrowLeftRight },
  { value: 'rules', label: 'Rules', icon: Workflow },
  { value: 'compose', label: 'Compose', icon: Send },
  { value: 'connect', label: 'Connect', icon: Plug },
];

/** Settings → General: theme, density, text size, layout, start page (UI prefs, apply instantly). */
export function GeneralSection() {
  const theme = useUiStore((s) => s.theme);
  const density = useUiStore((s) => s.density);
  const scale = useUiStore((s) => s.scale);
  const monoPaths = useUiStore((s) => s.monoPaths);
  const startPage = useUiStore((s) => s.startPage);
  const detailPosition = useUiStore((s) => s.detailPosition);
  const titleBar = useUiStore((s) => s.titleBar);
  const resolved = useResolvedTheme();
  const ui = useUiStore.getState();

  return (
    <>
      <SettingsCard id="appearance" title="Appearance" description="Applies instantly and syncs between the browser UI and the desktop app.">
        <SettingRow id="theme" title="Theme" description="System follows your operating system's light / dark setting." layout="stacked" control={<ThemePicker value={theme} onChange={ui.setTheme} resolved={resolved} />} />
        <SettingRow
          id="density"
          title="Density"
          description={`Row height in lists: ${DENSITIES.find((d) => d.id === density)?.rowHeight ?? 30} px.`}
          control={<SegmentedControl<Density> value={density} onValueChange={ui.setDensity} items={DENSITIES.map((d) => ({ value: d.id, label: d.label, tooltip: `${d.rowHeight} px rows` }))} aria-label="Density" />}
        />
        <SettingRow
          id="scale"
          title="Text size"
          description="Scales text across the interface."
          control={
            <SegmentedControl
              value={String(scale)}
              onValueChange={(v) => ui.setScale(Number(v) as UiScale)}
              items={UI_SCALES.map((s) => ({ value: String(s), label: `${Math.round(s * 100)}%` }))}
              aria-label="Text size"
            />
          }
        />
      </SettingsCard>

      <SettingsCard id="layout" title="Layout">
        <SettingRow
          id="mono-paths"
          title="Monospace paths"
          description="Show URLs and paths in the traffic list in a monospace font."
          control={<Switch checked={monoPaths} onCheckedChange={ui.setMonoPaths} aria-label="Monospace paths" />}
        />
        <SettingRow
          id="detail-position"
          title="Detail pane position"
          description="Auto puts it on the right on wide windows and below on narrow ones."
          control={
            <SegmentedControl<DetailPosition>
              value={detailPosition}
              onValueChange={ui.setDetailPosition}
              items={[
                { value: 'auto', label: 'Auto', icon: Sparkles },
                { value: 'right', label: 'Right', icon: PanelRight },
                { value: 'bottom', label: 'Bottom', icon: PanelBottom },
              ]}
              aria-label="Detail pane position"
            />
          }
        />
        {customTitleBarAvailable() && (
          <SettingRow
            id="title-bar"
            title="Title bar"
            description={
              window.__AKEV__?.platform === 'windows'
                ? 'Akev puts the window buttons in its own toolbar. System uses the standard Windows title bar.'
                : "Akev puts the window buttons in its own toolbar, in your desktop's button layout. System uses the standard title bar."
            }
            control={
              <SegmentedControl<TitleBarPref>
                value={titleBar}
                onValueChange={ui.setTitleBar}
                items={[
                  { value: 'akev', label: 'Akev', icon: PanelTop },
                  { value: 'system', label: 'System', icon: AppWindow },
                ]}
                aria-label="Title bar"
              />
            }
          />
        )}
        <SettingRow
          id="start-page"
          title="Start page"
          description="The page Akev opens on."
          control={<Select<StartPage> value={startPage} onValueChange={ui.setStartPage} options={START_PAGES.map((p) => ({ value: p.value, label: p.label, icon: p.icon }))} aria-label="Start page" className={styles.select} />}
        />
        <SettingRow
          id="setup-guide"
          title="Setup guide"
          description="Trust the certificate, launch a browser or terminal, and watch for your first request: the steps Akev shows on first launch."
          control={
            <Button
              icon={Rocket}
              onClick={() => {
                openFirstRun();
                navigate('/traffic');
              }}
            >
              Run the setup guide
            </Button>
          }
        />
      </SettingsCard>
    </>
  );
}
