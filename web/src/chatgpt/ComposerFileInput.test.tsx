import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { getAppLanguage, setAppLanguage, type AppLanguage } from '../i18n';
import { ComposerFileInput } from './ComposerFileInput';
import type { ChatGptTextAttachment } from './pasteAttachments';

const pickerMocks = vi.hoisted(() => ({
  pickFilePath: vi.fn(),
  pickProjectFolder: vi.fn(),
}));

vi.mock('../api', () => ({
  api: {
    pickFilePath: pickerMocks.pickFilePath,
    pickProjectFolder: pickerMocks.pickProjectFolder,
  },
}));

function Harness() {
  const [value, setValue] = useState('');
  const [attachments, setAttachments] = useState<ChatGptTextAttachment[]>([]);
  const [error, setError] = useState('');
  return <>
    <ComposerFileInput
      value={value}
      setValue={setValue}
      attachments={attachments}
      setAttachments={setAttachments}
      onPaste={() => undefined}
      onError={setError}
      placeholder="Type here"
      rows={2}
      variant="task"
    />
    <output data-testid="value">{value}</output>
    <output data-testid="attachments">{attachments.map((attachment) => attachment.name).join(',')}</output>
    <output data-testid="error">{error}</output>
  </>;
}

function dropData(files: File[], items: unknown[], plainText = '') {
  return {
    types: ['Files'],
    files,
    items,
    dropEffect: 'none',
    getData: (type: string) => type === 'text/plain' ? plainText : '',
  } as unknown as DataTransfer;
}

function fileItem(file: File) {
  return {
    getAsFile: () => file,
    webkitGetAsEntry: () => ({ isFile: true, isDirectory: false, name: file.name }),
  };
}

function folderItem(name: string) {
  return {
    getAsFile: () => null,
    webkitGetAsEntry: () => ({ isFile: false, isDirectory: true, name }),
  };
}

describe('ComposerFileInput', () => {
  let previousLanguage: AppLanguage;

  beforeEach(() => {
    previousLanguage = getAppLanguage();
    setAppLanguage('vi', false);
    pickerMocks.pickFilePath.mockReset();
    pickerMocks.pickProjectFolder.mockReset();
  });

  afterEach(() => setAppLanguage(previousLanguage, false));

  it('offers attach-file or attach-path when a file is dropped and inserts only the chosen path', async () => {
    render(<Harness />);
    const file = new File(['hello'], 'note.txt', { type: 'text/plain' });
    const textarea = screen.getByRole('textbox');

    fireEvent.drop(textarea.parentElement!, {
      dataTransfer: dropData([file], [fileItem(file)], 'D:\\docs\\note.txt'),
    });

    expect(await screen.findByRole('dialog', { name: 'Bạn muốn sử dụng tệp này như thế nào?' })).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Đính kèm path' }));

    await waitFor(() => expect(screen.getByTestId('value')).toHaveTextContent('D:\\docs\\note.txt'));
    expect(screen.getByTestId('attachments')).toBeEmptyDOMElement();
    expect(pickerMocks.pickFilePath).not.toHaveBeenCalled();
  });

  it('attaches a dropped file when the popup file option is chosen', async () => {
    render(<Harness />);
    const file = new File(['hello'], 'dropped.txt', { type: 'text/plain' });
    const textarea = screen.getByRole('textbox');

    fireEvent.drop(textarea.parentElement!, {
      dataTransfer: dropData([file], [fileItem(file)], 'D:\\docs\\dropped.txt'),
    });

    expect(await screen.findByRole('dialog', { name: 'Bạn muốn sử dụng tệp này như thế nào?' })).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Đính kèm tệp' }));

    await waitFor(() => expect(screen.getByTestId('attachments')).toHaveTextContent('dropped.txt'));
    expect(screen.getByTestId('value')).toBeEmptyDOMElement();
  });

  it('treats a dropped folder as a path without showing the file-choice popup', async () => {
    render(<Harness />);
    const textarea = screen.getByRole('textbox');

    fireEvent.drop(textarea.parentElement!, {
      dataTransfer: dropData([], [folderItem('Project')], 'D:\\DEV\\Project'),
    });

    await waitFor(() => expect(screen.getByTestId('value')).toHaveTextContent('D:\\DEV\\Project'));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(pickerMocks.pickProjectFolder).not.toHaveBeenCalled();
  });

  it('attaches files selected with the chooser', async () => {
    const view = render(<Harness />);
    const file = new File([new Uint8Array([1, 2, 3])], 'sample.bin', { type: 'application/octet-stream' });
    const input = view.container.querySelector<HTMLInputElement>('input[type="file"]');
    expect(input).not.toBeNull();

    fireEvent.change(input!, { target: { files: [file] } });

    await waitFor(() => expect(screen.getByTestId('attachments')).toHaveTextContent('sample.bin'));
    expect(screen.getByTestId('value')).toBeEmptyDOMElement();
    expect(screen.getByTestId('error')).toBeEmptyDOMElement();
  });
});
