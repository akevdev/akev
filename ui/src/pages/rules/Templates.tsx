import clsx from 'clsx';
import { ChevronDown, Plus, Upload } from 'lucide-react';
import { Button, DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, Kbd, Tooltip } from '../../components/ui';
import { BLANK_TEMPLATE, TEMPLATE_SECTIONS, TEMPLATES, templateById, type TemplateId } from '../../lib/rules/templates';
import { iconFor } from './icons';
import styles from './RulesPage.module.css';

const SECTION_HINT: Record<string, string> = {
  Respond: 'Answer instead of the server',
  Modify: 'Stack with other rules',
  Block: 'Simulate failures',
  WebSocket: 'Handle upgrades',
};

/** "New rule" split button: click = last template, chevron = template menu. */
export function NewRuleButton({ lastTemplate, onCreate, open, onOpenChange }: { lastTemplate: TemplateId; onCreate: (id: TemplateId) => void; open: boolean; onOpenChange: (open: boolean) => void }) {
  const last = templateById(lastTemplate) ?? TEMPLATES[0]!;
  return (
    <div className={styles.split}>
      <Tooltip content={`New rule: ${last.label}`} shortcut="n">
        <Button variant="primary" icon={Plus} onClick={() => onCreate(last.id)} className={styles.splitMain}>
          New rule
        </Button>
      </Tooltip>
      <DropdownMenu open={open} onOpenChange={onOpenChange} modal={false}>
        <Tooltip content="Start from a template" shortcut="alt+n">
          <DropdownMenuTrigger asChild>
            <Button variant="primary" className={styles.splitArrow} aria-label="Choose a template">
              <ChevronDown size={14} strokeWidth={2} aria-hidden />
            </Button>
          </DropdownMenuTrigger>
        </Tooltip>
        <DropdownMenuContent align="start" className={styles.templateMenu}>
          {TEMPLATE_SECTIONS.map((section, si) => (
            <div key={section} role="group" aria-label={section}>
              {si > 0 && <DropdownMenuSeparator />}
              <div className={styles.templateSection}>
                <span className={styles.templateSectionLabel}>{section}</span>
                <span className={styles.templateSectionHint}>{SECTION_HINT[section]}</span>
              </div>
              <div className={styles.templateGrid}>
                {TEMPLATES.filter((t) => t.section === section).map((t) => {
                  const Icon = iconFor(t.icon);
                  return (
                    <DropdownMenuItem key={t.id} className={styles.templateItem} onSelect={() => onCreate(t.id)}>
                      <span className={styles.templateIcon} data-section={t.section} aria-hidden>
                        <Icon size={15} strokeWidth={1.8} />
                      </span>
                      <span className={styles.templateText}>
                        <span className={styles.templateTitle}>
                          {t.label}
                          {t.id === lastTemplate && <Kbd combo="n" size="sm" variant="subtle" className={styles.templateKbd} />}
                        </span>
                        <span className={styles.templateDesc}>{t.description}</span>
                      </span>
                    </DropdownMenuItem>
                  );
                })}
              </div>
            </div>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem className={clsx(styles.templateItem, styles.templateBlank)} onSelect={() => onCreate(BLANK_TEMPLATE.id)}>
            <span className={styles.templateIcon} aria-hidden>
              {(() => {
                const Icon = iconFor(BLANK_TEMPLATE.icon);
                return <Icon size={15} strokeWidth={1.8} />;
              })()}
            </span>
            <span className={styles.templateText}>
              <span className={styles.templateTitle}>{BLANK_TEMPLATE.label}</span>
              <span className={styles.templateDesc}>{BLANK_TEMPLATE.description}</span>
            </span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

/** Empty state: what rules can do, as one-click template cards. */
export function TemplateGallery({ onCreate, onImport, lastTemplate }: { onCreate: (id: TemplateId) => void; onImport: () => void; lastTemplate: TemplateId }) {
  return (
    <div className={styles.gallery}>
      <div className={styles.galleryInner}>
        <div className={styles.galleryHero}>
          <span className={styles.galleryBadge}>Rules</span>
          <h2 className={styles.galleryTitle}>Mock, rewrite, delay and pause traffic</h2>
          <p className={styles.galleryText}>
            Rules match requests flowing through Akev and decide what happens to them. They run top to bottom: rewrites, delays and breakpoints stack, and the first rule that responds wins.
          </p>
          <div className={styles.galleryActions}>
            <Button variant="primary" icon={Plus} kbd={lastTemplate === 'mock' ? 'n' : undefined} onClick={() => onCreate('mock')}>
              Mock a response
            </Button>
            <Button icon={Upload} onClick={onImport}>
              Import rules…
            </Button>
          </div>
        </div>
        {TEMPLATE_SECTIONS.map((section) => (
          <section key={section} className={styles.gallerySection} aria-label={section}>
            <div className={styles.templateSection}>
              <span className={styles.templateSectionLabel}>{section}</span>
              <span className={styles.templateSectionHint}>{SECTION_HINT[section]}</span>
            </div>
            <div className={styles.galleryGrid}>
              {TEMPLATES.filter((t) => t.section === section).map((t) => {
                const Icon = iconFor(t.icon);
                return (
                  <button key={t.id} type="button" className={styles.galleryCard} onClick={() => onCreate(t.id)}>
                    <span className={styles.templateIcon} data-section={t.section} aria-hidden>
                      <Icon size={16} strokeWidth={1.8} />
                    </span>
                    <span className={styles.templateText}>
                      <span className={styles.templateTitle}>
                        {t.label}
                        {t.id === lastTemplate && lastTemplate !== 'mock' && <Kbd combo="n" size="sm" variant="subtle" className={styles.templateKbd} />}
                      </span>
                      <span className={styles.templateDesc}>{t.description}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          </section>
        ))}
        <p className={styles.galleryFoot}>
          Already use HTTP Toolkit? Import your <code>.htkrules</code> export — Akev converts it.
        </p>
      </div>
    </div>
  );
}
