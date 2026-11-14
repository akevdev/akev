import { ChevronDown, File, Folder, FolderOpen } from 'lucide-react';
import { errorMessage } from '../../../api/client';
import { Button, Menu, toast } from '../../../components/ui';
import { canPickNativePath, pickNativePath } from '../../../lib/platform';

export interface PathPickerProps {
  /** What may be picked: a file, a folder, or either (a menu asks which). */
  pick: 'file' | 'folder' | 'either';
  onPick: (path: string) => void;
  /** Dialog title. */
  title?: string;
}

/**
 * "Choose…" next to a path field: the native file / folder dialog of the desktop app. Hidden in
 * the browser, which never reveals where a file lives (the path is read by Akev, on disk).
 */
export function PathPicker({ pick, onPick, title }: PathPickerProps) {
  if (!canPickNativePath()) return null;
  const choose = async (directory: boolean) => {
    try {
      const path = await pickNativePath({ directory, title });
      if (path) onPick(path);
    } catch (e) {
      toast.error(`Couldn't open the ${directory ? 'folder' : 'file'} picker`, { description: `${errorMessage(e)}. Type or paste the path instead.` });
    }
  };
  if (pick !== 'either') {
    return (
      <Button icon={pick === 'folder' ? FolderOpen : File} onClick={() => void choose(pick === 'folder')}>
        Choose…
      </Button>
    );
  }
  return (
    <Menu
      trigger={
        <Button icon={FolderOpen} iconRight={ChevronDown}>
          Choose…
        </Button>
      }
      items={[
        { label: 'A file…', icon: File, onSelect: () => void choose(false) },
        { label: 'A folder…', icon: Folder, onSelect: () => void choose(true) },
      ]}
    />
  );
}
