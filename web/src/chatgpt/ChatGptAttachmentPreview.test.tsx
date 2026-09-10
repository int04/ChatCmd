import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { ChatGptAttachmentPreview, attachmentExtension, attachmentImageDataUrl } from './ChatGptAttachmentPreview';
import type { ChatGptTextAttachment } from './pasteAttachments';

function attachment(overrides: Partial<ChatGptTextAttachment> = {}): ChatGptTextAttachment {
  return {
    id: 'file-1',
    name: 'report.docx',
    content: 'AAEC',
    mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    encoding: 'base64',
    sizeBytes: 3,
    ...overrides,
  };
}

describe('ChatGptAttachmentPreview', () => {
  it('renders clipboard images as a real thumbnail', () => {
    const image = attachment({
      name: 'clipboard-image-1.png',
      content: 'iVBORw0KGgo=',
      mimeType: 'image/png',
    });
    const view = render(<ChatGptAttachmentPreview attachment={image} onRemove={() => undefined} />);

    const img = view.container.querySelector('img');
    expect(img).not.toBeNull();
    expect(img).toHaveAttribute('src', 'data:image/png;base64,iVBORw0KGgo=');
    expect(screen.getByText('PNG · 3 B')).toBeVisible();

    fireEvent.click(screen.getByRole('button', { name: 'Preview image clipboard-image-1.png' }));
    expect(screen.getByRole('dialog', { name: 'clipboard-image-1.png' })).toBeVisible();
    expect(screen.getByRole('img', { name: 'clipboard-image-1.png' })).toHaveAttribute('src', 'data:image/png;base64,iVBORw0KGgo=');

    fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
    expect(screen.queryByRole('dialog', { name: 'clipboard-image-1.png' })).not.toBeInTheDocument();
  });

  it('renders non-image files with an uppercase extension badge', () => {
    render(<ChatGptAttachmentPreview attachment={attachment()} onRemove={() => undefined} />);

    expect(screen.getByText('DOCX')).toBeVisible();
    expect(screen.getByText('DOCX · 3 B')).toBeVisible();
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
  });

  it('falls back to FILE when the name has no extension', () => {
    render(<ChatGptAttachmentPreview attachment={attachment({ name: 'LICENSE', mimeType: 'application/octet-stream' })} onRemove={() => undefined} />);
    expect(screen.getByText('FILE')).toBeVisible();
  });

  it('removes the selected attachment', () => {
    const onRemove = vi.fn();
    render(<ChatGptAttachmentPreview attachment={attachment()} onRemove={onRemove} />);

    fireEvent.click(screen.getByRole('button'));
    expect(onRemove).toHaveBeenCalledOnce();
  });

  it('does not build image data URLs for non-image or UTF-8 attachments', () => {
    expect(attachmentImageDataUrl(attachment())).toBeUndefined();
    expect(attachmentImageDataUrl(attachment({ mimeType: 'image/png', encoding: 'utf8' }))).toBeUndefined();
  });

  it('extracts normalized file extensions without treating dotfiles as extensions', () => {
    expect(attachmentExtension('D:\\docs\\archive.tar.gz')).toBe('GZ');
    expect(attachmentExtension('.gitignore')).toBe('');
    expect(attachmentExtension('README')).toBe('');
  });
});
