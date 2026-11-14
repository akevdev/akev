/**
 * Akev UI primitives (docs/DESIGN.md §5). Import from here:
 * `import { Button, IconButton, Tooltip, toast } from '@/components/ui'` (relative path in practice).
 */

export { AppIcon, APP_ICON_NAMES, appIconFor, type AppIconName, type AppIconProps } from './AppIcon';
export { Badge, type BadgeProps, type BadgeVariant } from './Badge';
export { Button, type ButtonProps, type ButtonVariant, type ControlSize } from './Button';
export { Checkbox, type CheckboxProps } from './Checkbox';
export { CodeEditor, normalizeLanguage, type CodeEditorHandle, type CodeEditorProps, type CodeLanguage } from './CodeEditor';
export { ColorDot, type ColorDotProps } from './ColorDot';
export { Combobox, type ComboboxOption, type ComboboxProps } from './Combobox';
export { confirm, ConfirmHost, type ConfirmOptions } from './confirm';
export {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItems,
  ContextMenuRoot,
  ContextMenuTrigger,
  type ContextMenuProps,
} from './ContextMenu';
export { CopyButton, type CopyButtonProps } from './CopyButton';
export { Dialog, DialogClose, Sheet, type DialogProps, type SheetProps } from './Dialog';
export {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuItems,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
  Menu,
  type MenuProps,
} from './DropdownMenu';
export { EmptyState, type EmptyStateProps } from './EmptyState';
export { Field, type FieldProps } from './Field';
export { HexViewer, type HexViewerProps } from './HexViewer';
export { IconButton, type IconButtonProps } from './IconButton';
export { Input, type InputProps } from './Input';
export { JsonTree, type JsonTreeProps } from './JsonTree';
export { Kbd, type KbdProps } from './Kbd';
export {
  formatBulk,
  headersToRows,
  KeyValueEditor,
  makeRow,
  parseBulk,
  rowsToHeaders,
  type KeyValueEditorProps,
  type KeyValueRow,
} from './KeyValueEditor';
export { afterMenuClose, MenuButton, SplitMenuButton, type MenuButtonProps, type SplitMenuButtonProps } from './MenuButton';
export type { MenuItems, MenuItemSpec } from './menuTypes';
export { MethodBadge, type MethodBadgeProps } from './MethodBadge';
export { NumberInput, type NumberInputProps } from './NumberInput';
export { Popover, PopoverAnchor, PopoverClose, PopoverContent, PopoverTrigger, type PopoverContentProps } from './Popover';
export { ProgressBar, type ProgressBarProps } from './ProgressBar';
export { QrCode, type QrCodeProps } from './QrCode';
export { SearchInput, type SearchInputProps } from './SearchInput';
export { Section, type SectionProps } from './Section';
export { SegmentedControl, type SegmentedControlProps, type SegmentedItem } from './SegmentedControl';
export { Select, type SelectGroup, type SelectOption, type SelectProps } from './Select';
export { Skeleton, type SkeletonProps } from './Skeleton';
export { Slider, type SliderProps } from './Slider';
export { Spinner, type SpinnerProps } from './Spinner';
export { SplitPane, type SplitPaneProps } from './SplitPane';
export { StatusBadge, type StatusBadgeProps } from './StatusBadge';
export { Switch, type SwitchProps } from './Switch';
export { TabList, Tabs, TabsContent, type TabItem, type TabListProps, type TabsProps } from './Tabs';
export { TextArea, type TextAreaProps } from './TextArea';
export { toast, Toaster, undoToast } from './toast';
export { Toolbar, ToolbarSeparator, type ToolbarProps } from './Toolbar';
export { Tooltip, TooltipProvider, type TooltipProps } from './Tooltip';
