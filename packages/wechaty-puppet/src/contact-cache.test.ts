import { it } from 'node:test'
import assert from 'node:assert/strict'
import type { Chat, Contact, WeChatClient } from '@agent-wechat/shared'
import * as PUPPET from 'wechaty-puppet'
import { PuppetAgentWeChat } from './puppet-agent-wechat.js'
import { contactToContactPayload } from './type-map.js'

it('polling preserves contact friendship and keeps chat-only fallback behavior', async () => {
  const puppet = new PuppetAgentWeChat({ token: 'fixture-token' })
  Object.defineProperty(puppet, 'currentUserId', { value: 'self' })
  const internals = puppet as unknown as {
    client: WeChatClient
    contactStore: Map<string, PUPPET.payloads.Contact>
    pollMessages(): Promise<void>
  }
  const contact: Contact = { username: 'stranger', nickName: 'Alice', contactType: 'individual', localType: 3 }
  internals.contactStore.set(contact.username, contactToContactPayload(contact))
  const chats: Chat[] = ['stranger', 'chat-only'].map(username => ({
    id: username, username, name: 'Updated name', unreadCount: 0, isGroup: false,
  }))
  internals.client = {
    authStatus: async () => ({ status: 'logged_in', loggedInUser: 'self' }),
    listChats: async () => chats,
  } as unknown as WeChatClient
  await internals.pollMessages()
  assert.equal(internals.contactStore.get('stranger')?.friend, false)
  assert.equal(internals.contactStore.get('stranger')?.name, 'Updated name')
  assert.equal(internals.contactStore.get('chat-only')?.friend, true)
})
